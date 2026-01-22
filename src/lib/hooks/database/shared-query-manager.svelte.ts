import type { SharedQuery, QueryParameter } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { SharedRepoManager } from './shared-repo-manager.svelte.js';
import {
	parseQueryFile,
	serializeQueryFile,
	queryNameToFilename,
	isValidQueryPath
} from '$lib/services/query-file-parser';
import {
	readTextFile,
	writeTextFile,
	remove,
	mkdir,
	exists,
	rename
} from '@tauri-apps/plugin-fs';
import { join, dirname } from '@tauri-apps/api/path';
import * as gitService from '$lib/services/git';

/**
 * Manages individual shared queries: create, update, delete.
 */
export class SharedQueryManager {
	private _autoCommit: boolean;

	constructor(
		private state: DatabaseState,
		private repoManager: SharedRepoManager,
		autoCommit: boolean = true
	) {
		this._autoCommit = autoCommit;
	}

	/**
	 * Create a new shared query in the active repository.
	 */
	async createQuery(
		name: string,
		query: string,
		folder: string = '',
		options?: {
			description?: string;
			databaseType?: string;
			tags?: string[];
			parameters?: QueryParameter[];
		}
	): Promise<string | null> {
		const repoId = this.state.activeRepoId;
		if (!repoId) return null;

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return null;

		// Generate file path
		const filename = queryNameToFilename(name);
		const filePath = folder ? `${folder}/${filename}` : filename;

		if (!isValidQueryPath(filePath)) {
			throw new Error('Invalid query file path');
		}

		// Create the SharedQuery object
		const sharedQuery: SharedQuery = {
			id: `${repoId}:${filePath}`,
			repoId,
			filePath,
			name,
			description: options?.description,
			query,
			parameters: options?.parameters,
			databaseType: options?.databaseType,
			tags: options?.tags ?? [],
			folder
		};

		// Serialize to file content
		const content = serializeQueryFile(sharedQuery);

		// Ensure folder exists
		const fullPath = await join(repo.path, filePath);
		const folderPath = await dirname(fullPath);

		if (!(await exists(folderPath))) {
			await mkdir(folderPath, { recursive: true });
		}

		// Write file
		await writeTextFile(fullPath, content);

		// Add to state
		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		this.state.sharedQueriesByRepo = {
			...this.state.sharedQueriesByRepo,
			[repoId]: [...queries, sharedQuery]
		};

		// Auto-commit if enabled
		if (this._autoCommit) {
			await gitService.stageFile(repo.path, filePath);
			await gitService.commitChanges(repo.path, `Add query: ${name}`);
		}

		await this.repoManager.refreshRepoStatus(repoId);
		return sharedQuery.id;
	}

	/**
	 * Update an existing shared query.
	 */
	async updateQuery(
		queryId: string,
		updates: {
			name?: string;
			query?: string;
			description?: string;
			databaseType?: string;
			tags?: string[];
			parameters?: QueryParameter[];
		}
	): Promise<boolean> {
		// Find the query
		const [repoId, ...pathParts] = queryId.split(':');
		const filePath = pathParts.join(':');

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return false;

		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		const queryIndex = queries.findIndex((q) => q.id === queryId);
		if (queryIndex === -1) return false;

		const existingQuery = queries[queryIndex];

		// Create updated query
		const updatedQuery: SharedQuery = {
			...existingQuery,
			name: updates.name ?? existingQuery.name,
			query: updates.query ?? existingQuery.query,
			description: updates.description ?? existingQuery.description,
			databaseType: updates.databaseType ?? existingQuery.databaseType,
			tags: updates.tags ?? existingQuery.tags,
			parameters: updates.parameters ?? existingQuery.parameters
		};

		// Handle rename (file path change)
		let newFilePath = filePath;
		if (updates.name && updates.name !== existingQuery.name) {
			const folder = existingQuery.folder;
			const newFilename = queryNameToFilename(updates.name);
			newFilePath = folder ? `${folder}/${newFilename}` : newFilename;

			if (newFilePath !== filePath) {
				// Rename file
				const oldFullPath = await join(repo.path, filePath);
				const newFullPath = await join(repo.path, newFilePath);

				await rename(oldFullPath, newFullPath);

				// Update query object
				updatedQuery.id = `${repoId}:${newFilePath}`;
				updatedQuery.filePath = newFilePath;

				// Stage the rename
				if (this._autoCommit) {
					await gitService.stageFile(repo.path, filePath);
					await gitService.stageFile(repo.path, newFilePath);
				}
			}
		}

		// Serialize and write updated content
		const content = serializeQueryFile(updatedQuery);
		const fullPath = await join(repo.path, updatedQuery.filePath);
		await writeTextFile(fullPath, content);

		// Update state
		const updatedQueries = [...queries];
		updatedQueries[queryIndex] = updatedQuery;
		this.state.sharedQueriesByRepo = {
			...this.state.sharedQueriesByRepo,
			[repoId]: updatedQueries
		};

		// Auto-commit if enabled
		if (this._autoCommit) {
			await gitService.stageFile(repo.path, updatedQuery.filePath);
			await gitService.commitChanges(repo.path, `Update query: ${updatedQuery.name}`);
		}

		await this.repoManager.refreshRepoStatus(repoId);
		return true;
	}

	/**
	 * Delete a shared query.
	 */
	async deleteQuery(queryId: string): Promise<boolean> {
		const [repoId, ...pathParts] = queryId.split(':');
		const filePath = pathParts.join(':');

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return false;

		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		const query = queries.find((q) => q.id === queryId);
		if (!query) return false;

		// Delete file
		const fullPath = await join(repo.path, filePath);
		await remove(fullPath);

		// Remove from state
		this.state.sharedQueriesByRepo = {
			...this.state.sharedQueriesByRepo,
			[repoId]: queries.filter((q) => q.id !== queryId)
		};

		// Auto-commit if enabled
		if (this._autoCommit) {
			await gitService.stageFile(repo.path, filePath);
			await gitService.commitChanges(repo.path, `Delete query: ${query.name}`);
		}

		await this.repoManager.refreshRepoStatus(repoId);
		return true;
	}

	/**
	 * Move a query to a different folder.
	 */
	async moveQuery(queryId: string, newFolder: string): Promise<boolean> {
		const [repoId, ...pathParts] = queryId.split(':');
		const filePath = pathParts.join(':');

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return false;

		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		const queryIndex = queries.findIndex((q) => q.id === queryId);
		if (queryIndex === -1) return false;

		const query = queries[queryIndex];
		const filename = filePath.split('/').pop() || '';
		const newFilePath = newFolder ? `${newFolder}/${filename}` : filename;

		if (!isValidQueryPath(newFilePath)) {
			throw new Error('Invalid target folder');
		}

		// Ensure target folder exists
		const targetFolderPath = await join(repo.path, newFolder);
		if (newFolder && !(await exists(targetFolderPath))) {
			await mkdir(targetFolderPath, { recursive: true });
		}

		// Move file
		const oldFullPath = await join(repo.path, filePath);
		const newFullPath = await join(repo.path, newFilePath);
		await rename(oldFullPath, newFullPath);

		// Update query object
		const updatedQuery: SharedQuery = {
			...query,
			id: `${repoId}:${newFilePath}`,
			filePath: newFilePath,
			folder: newFolder
		};

		// Update state
		const updatedQueries = [...queries];
		updatedQueries[queryIndex] = updatedQuery;
		this.state.sharedQueriesByRepo = {
			...this.state.sharedQueriesByRepo,
			[repoId]: updatedQueries
		};

		// Auto-commit if enabled
		if (this._autoCommit) {
			await gitService.stageFile(repo.path, filePath);
			await gitService.stageFile(repo.path, newFilePath);
			await gitService.commitChanges(repo.path, `Move query: ${query.name}`);
		}

		await this.repoManager.refreshRepoStatus(repoId);
		return true;
	}

	/**
	 * Create a new folder in the repository.
	 */
	async createFolder(folderPath: string): Promise<boolean> {
		const repoId = this.state.activeRepoId;
		if (!repoId) return false;

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return false;

		const fullPath = await join(repo.path, folderPath);

		if (await exists(fullPath)) {
			return false; // Already exists
		}

		await mkdir(fullPath, { recursive: true });

		// Create .gitkeep to track empty folder
		const gitkeepPath = await join(fullPath, '.gitkeep');
		await writeTextFile(gitkeepPath, '');

		if (this._autoCommit) {
			await gitService.stageFile(repo.path, `${folderPath}/.gitkeep`);
			await gitService.commitChanges(repo.path, `Create folder: ${folderPath}`);
		}

		return true;
	}

	/**
	 * Get a shared query by ID.
	 */
	getQuery(queryId: string): SharedQuery | null {
		const [repoId] = queryId.split(':');
		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		return queries.find((q) => q.id === queryId) ?? null;
	}

	/**
	 * Get all queries in a specific folder.
	 */
	getQueriesInFolder(repoId: string, folder: string): SharedQuery[] {
		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		return queries.filter((q) => q.folder === folder);
	}

	/**
	 * Get all unique folder paths in a repository.
	 */
	getFolders(repoId: string): string[] {
		const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		const folders = new Set<string>();

		for (const query of queries) {
			if (query.folder) {
				// Add the folder and all parent folders
				const parts = query.folder.split('/');
				for (let i = 1; i <= parts.length; i++) {
					folders.add(parts.slice(0, i).join('/'));
				}
			}
		}

		return Array.from(folders).sort();
	}

	/**
	 * Search queries across all repositories.
	 */
	searchQueries(searchTerm: string, repoId?: string): SharedQuery[] {
		const term = searchTerm.toLowerCase();
		let queries: SharedQuery[];

		if (repoId) {
			queries = this.state.sharedQueriesByRepo[repoId] ?? [];
		} else {
			queries = this.state.allSharedQueries;
		}

		return queries.filter((q) => {
			return (
				q.name.toLowerCase().includes(term) ||
				q.description?.toLowerCase().includes(term) ||
				q.query.toLowerCase().includes(term) ||
				q.tags.some((t) => t.toLowerCase().includes(term))
			);
		});
	}

	/**
	 * Reload a single query from disk.
	 */
	async reloadQuery(queryId: string): Promise<void> {
		const [repoId, ...pathParts] = queryId.split(':');
		const filePath = pathParts.join(':');

		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return;

		const fullPath = await join(repo.path, filePath);

		try {
			const content = await readTextFile(fullPath);
			const query = parseQueryFile(content, repoId, filePath);

			if (query) {
				const queries = this.state.sharedQueriesByRepo[repoId] ?? [];
				const index = queries.findIndex((q) => q.id === queryId);

				if (index !== -1) {
					const updatedQueries = [...queries];
					updatedQueries[index] = query;
					this.state.sharedQueriesByRepo = {
						...this.state.sharedQueriesByRepo,
						[repoId]: updatedQueries
					};
				}
			}
		} catch (error) {
			console.error('Failed to reload query:', error);
		}
	}

	/**
	 * Set auto-commit behavior.
	 */
	setAutoCommit(enabled: boolean): void {
		this._autoCommit = enabled;
	}
}
