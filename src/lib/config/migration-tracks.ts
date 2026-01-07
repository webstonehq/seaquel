import type { UserBackground } from "$lib/stores/onboarding.svelte.js";

export interface MigrationHint {
	id: string;
	title: string;
	description: string;
	location: "sidebar" | "editor" | "results" | "header";
}

export interface UIMapping {
	theirTerm: string;
	ourTerm: string;
}

export interface MigrationTrack {
	id: UserBackground;
	name: string;
	logo: string;
	welcomeMessage: string;
	welcomeDescription: string;
	keyboardNote: string;
	uiMappings: UIMapping[];
	hints: MigrationHint[];
}

export const migrationTracks: Record<Exclude<UserBackground, "none">, MigrationTrack> = {
	datagrip: {
		id: "datagrip",
		name: "DataGrip",
		logo: "datagrip",
		welcomeMessage: "Welcome from DataGrip!",
		welcomeDescription:
			"Your keyboard shortcuts work the same here. Let's help you find your way around.",
		keyboardNote: "Your shortcuts are the same: Cmd+Enter to execute, Cmd+S to save",
		uiMappings: [
			{ theirTerm: "Database Explorer", ourTerm: "Schema Browser (left sidebar)" },
			{ theirTerm: "Console", ourTerm: "Query Editor" },
			{ theirTerm: "Services", ourTerm: "Connections (header dropdown)" },
			{ theirTerm: "Data Editor", ourTerm: "Results Table" },
		],
		hints: [
			{
				id: "datagrip-schema",
				title: "Schema Browser",
				description:
					"Your Database Explorer is in the left sidebar. Click the Schema tab to browse tables.",
				location: "sidebar",
			},
			{
				id: "datagrip-connections",
				title: "Connections",
				description:
					"Switch between databases using the dropdown in the header, not a sidebar tree.",
				location: "header",
			},
		],
	},
	dbeaver: {
		id: "dbeaver",
		name: "DBeaver",
		logo: "dbeaver",
		welcomeMessage: "Welcome from DBeaver!",
		welcomeDescription:
			"Seaquel has a simpler interface focused on what matters. Here's what's different.",
		keyboardNote: "Cmd+Enter executes queries, just like you're used to",
		uiMappings: [
			{ theirTerm: "Database Navigator", ourTerm: "Schema tab (left sidebar)" },
			{ theirTerm: "SQL Editor", ourTerm: "Query tabs" },
			{ theirTerm: "Projects", ourTerm: "Saved Queries (no projects needed)" },
			{ theirTerm: "ER Diagrams", ourTerm: "ERD Viewer (header button)" },
		],
		hints: [
			{
				id: "dbeaver-navigator",
				title: "Database Navigator",
				description: "Find your tables in the Schema tab on the left sidebar.",
				location: "sidebar",
			},
			{
				id: "dbeaver-erd",
				title: "ERD Diagrams",
				description: "Click the network icon in the header to view entity relationship diagrams.",
				location: "header",
			},
		],
	},
};

export const getMigrationTrack = (
	background: UserBackground
): MigrationTrack | null => {
	if (background === "none") return null;
	return migrationTracks[background] ?? null;
};
