import type { DatabaseType, SSHAuthMethod } from "$lib/types";
import type { ConnectionDialogPrefill } from "./connection-dialog.svelte.js";

export type WizardStep =
	| "string-choice"
	| "string-paste"
	| "type"
	| "host"
	| "credentials"
	| "advanced"
	| "success";

export type WizardMode = "wizard" | "quick" | "reconnect";

export interface WizardFormData {
	name: string;
	type: DatabaseType;
	host: string;
	port: number;
	databaseName: string;
	username: string;
	password: string;
	sslMode: string;
	connectionString: string;
	// SSH Tunnel fields
	sshEnabled: boolean;
	sshHost: string;
	sshPort: number;
	sshUsername: string;
	sshAuthMethod: SSHAuthMethod;
	sshPassword: string;
	sshKeyPath: string;
	sshKeyPassphrase: string;
}

export interface DatabaseTypeConfig {
	value: DatabaseType;
	label: string;
	defaultPort: number;
	protocol: string[];
	description: string;
	icon: string;
}

export const databaseTypes: DatabaseTypeConfig[] = [
	{
		value: "postgres",
		label: "PostgreSQL",
		defaultPort: 5432,
		protocol: ["postgres", "postgresql"],
		description: "The world's most advanced open-source database",
		icon: "postgresql",
	},
	{
		value: "mysql",
		label: "MySQL",
		defaultPort: 3306,
		protocol: ["mysql"],
		description: "Popular open-source relational database",
		icon: "mysql",
	},
	{
		value: "mariadb",
		label: "MariaDB",
		defaultPort: 3306,
		protocol: ["mariadb"],
		description: "Community-developed fork of MySQL",
		icon: "mariadb",
	},
	{
		value: "sqlite",
		label: "SQLite",
		defaultPort: 0,
		protocol: ["sqlite"],
		description: "Lightweight file-based database",
		icon: "sqlite",
	},
	{
		value: "mongodb",
		label: "MongoDB",
		defaultPort: 27017,
		protocol: ["mongodb", "mongodb+srv"],
		description: "Document-oriented NoSQL database",
		icon: "mongodb",
	},
	{
		value: "mssql",
		label: "SQL Server",
		defaultPort: 1433,
		protocol: ["mssql", "sqlserver"],
		description: "Microsoft's enterprise database",
		icon: "mssql",
	},
];

const defaultFormData: WizardFormData = {
	name: "",
	type: "postgres",
	host: "localhost",
	port: 5432,
	databaseName: "",
	username: "",
	password: "",
	sslMode: "disable",
	connectionString: "",
	sshEnabled: false,
	sshHost: "",
	sshPort: 22,
	sshUsername: "",
	sshAuthMethod: "password",
	sshPassword: "",
	sshKeyPath: "",
	sshKeyPassphrase: "",
};

// Step order for manual flow (after choosing "No" for connection string)
const manualStepOrder: WizardStep[] = ["type", "host", "credentials", "advanced", "success"];

// Step order for connection string flow (after choosing "Yes")
const stringStepOrder: WizardStep[] = ["string-paste", "success"];

class ConnectionWizardStore {
	isOpen = $state(false);
	mode = $state<WizardMode>("wizard");
	currentStep = $state<WizardStep>("string-choice");
	formData = $state<WizardFormData>({ ...defaultFormData });

	// Reconnection context
	reconnectingConnectionId = $state<string | null>(null);

	// Connection state
	isConnecting = $state(false);
	isTesting = $state(false);
	connectionError = $state<string | null>(null);
	connectionSuccess = $state(false);

	// Track which flow path the user chose
	private usingConnectionString = $state(false);

	// Derived: current step number for progress indicator
	get stepNumber(): number {
		if (this.currentStep === "string-choice") return 0;

		const steps = this.usingConnectionString ? stringStepOrder : manualStepOrder;
		const index = steps.indexOf(this.currentStep);
		return index >= 0 ? index + 1 : 0;
	}

	// Derived: total steps for progress indicator
	get totalSteps(): number {
		if (this.currentStep === "string-choice") return 0;
		return this.usingConnectionString ? stringStepOrder.length : manualStepOrder.length;
	}

	// Derived: selected database type config
	get selectedDbType(): DatabaseTypeConfig | undefined {
		return databaseTypes.find((t) => t.value === this.formData.type);
	}

	// Derived: can proceed to next step
	get canProceed(): boolean {
		switch (this.currentStep) {
			case "string-choice":
				return true;
			case "string-paste":
				return this.formData.connectionString.trim().length > 0;
			case "type":
				return true; // Type is always selected
			case "host":
				if (this.formData.type === "sqlite") return true;
				return this.formData.host.trim().length > 0;
			case "credentials":
				return (
					this.formData.databaseName.trim().length > 0 &&
					this.formData.name.trim().length > 0
				);
			case "advanced":
				return true; // Advanced is optional
			case "success":
				return true;
			default:
				return false;
		}
	}

	// Open the wizard
	open(mode: WizardMode = "wizard", prefill?: ConnectionDialogPrefill): void {
		this.mode = mode;
		this.connectionError = null;
		this.connectionSuccess = false;
		this.isConnecting = false;
		this.isTesting = false;

		if (prefill) {
			// Reconnecting to existing connection
			this.formData = {
				name: prefill.name || "",
				type: (prefill.type as DatabaseType) || "postgres",
				host: prefill.host || "localhost",
				port: prefill.port || 5432,
				databaseName: prefill.databaseName || "",
				username: prefill.username || "",
				password: "", // Always empty for security
				sslMode: prefill.sslMode || "disable",
				connectionString: prefill.connectionString || "",
				sshEnabled: prefill.sshTunnel?.enabled || false,
				sshHost: prefill.sshTunnel?.host || "",
				sshPort: prefill.sshTunnel?.port || 22,
				sshUsername: prefill.sshTunnel?.username || "",
				sshAuthMethod: prefill.sshTunnel?.authMethod || "password",
				sshPassword: "",
				sshKeyPath: "",
				sshKeyPassphrase: "",
			};
			this.reconnectingConnectionId = prefill.id || null;

			if (mode === "reconnect") {
				// For reconnect, go straight to credentials
				this.currentStep = "credentials";
				this.usingConnectionString = false;
			} else if (prefill.connectionString) {
				this.usingConnectionString = true;
				this.currentStep = "string-paste";
			} else {
				this.currentStep = "string-choice";
				this.usingConnectionString = false;
			}
		} else {
			// New connection
			this.formData = { ...defaultFormData };
			this.reconnectingConnectionId = null;
			this.currentStep = mode === "quick" ? "type" : "string-choice";
			this.usingConnectionString = false;
		}

		this.isOpen = true;
	}

	// Close the wizard
	close(): void {
		this.isOpen = false;
		this.formData = { ...defaultFormData };
		this.reconnectingConnectionId = null;
		this.connectionError = null;
		this.connectionSuccess = false;
		this.currentStep = "string-choice";
		this.usingConnectionString = false;
	}

	// Choose to use connection string (from string-choice step)
	chooseConnectionString(): void {
		this.usingConnectionString = true;
		this.currentStep = "string-paste";
	}

	// Choose manual entry (from string-choice step)
	chooseManual(): void {
		this.usingConnectionString = false;
		this.currentStep = "type";
	}

	// Navigate to next step
	nextStep(): void {
		const steps = this.usingConnectionString ? stringStepOrder : manualStepOrder;
		const currentIndex = steps.indexOf(this.currentStep);

		if (currentIndex >= 0 && currentIndex < steps.length - 1) {
			// Skip host step for SQLite
			let nextIndex = currentIndex + 1;
			if (steps[nextIndex] === "host" && this.formData.type === "sqlite") {
				nextIndex++;
			}
			this.currentStep = steps[nextIndex];
		}
	}

	// Navigate to previous step
	prevStep(): void {
		if (this.currentStep === "string-choice") return;

		// If at first step of a flow, go back to choice
		if (this.currentStep === "string-paste" || this.currentStep === "type") {
			this.currentStep = "string-choice";
			this.usingConnectionString = false;
			return;
		}

		const steps = this.usingConnectionString ? stringStepOrder : manualStepOrder;
		const currentIndex = steps.indexOf(this.currentStep);

		if (currentIndex > 0) {
			// Skip host step for SQLite when going back
			let prevIndex = currentIndex - 1;
			if (steps[prevIndex] === "host" && this.formData.type === "sqlite") {
				prevIndex--;
			}
			if (prevIndex >= 0) {
				this.currentStep = steps[prevIndex];
			}
		}
	}

	// Go directly to a specific step
	goToStep(step: WizardStep): void {
		this.currentStep = step;
	}

	// Set database type and update port
	setDatabaseType(type: DatabaseType): void {
		this.formData.type = type;
		const dbType = databaseTypes.find((t) => t.value === type);
		if (dbType) {
			this.formData.port = dbType.defaultPort;
		}
	}

	// Parse connection string and populate form data
	parseConnectionString(connStr: string): boolean {
		try {
			// Handle SQLite
			if (connStr.startsWith("sqlite://") || connStr.startsWith("sqlite:")) {
				const dbPath = connStr.replace(/^sqlite:\/\//, "").replace(/^sqlite:/, "");
				this.formData.type = "sqlite";
				this.formData.databaseName = dbPath;
				this.formData.name = `SQLite - ${dbPath.split("/").pop() || "database"}`;
				return true;
			}

			// Normalize postgresql to postgres
			connStr = connStr.replace("postgresql://", "postgres://");

			// Parse as URL
			const url = new URL(connStr);
			const protocol = url.protocol.replace(":", "");
			const dbType = databaseTypes.find((t) => t.protocol.includes(protocol));

			if (!dbType) {
				this.connectionError = `Unsupported database type: ${protocol}`;
				return false;
			}

			this.formData.type = dbType.value;
			this.formData.host = url.hostname;
			this.formData.port = url.port ? parseInt(url.port) : dbType.defaultPort;
			this.formData.databaseName = url.pathname.replace(/^\//, "");
			this.formData.username = url.username ? decodeURIComponent(url.username) : "";
			this.formData.password = url.password ? decodeURIComponent(url.password) : "";

			// Parse SSL mode from query parameters
			const sslModeParam = url.searchParams.get("sslmode");
			if (sslModeParam) {
				this.formData.sslMode = sslModeParam;
			}

			// Generate name from database
			this.formData.name = this.formData.databaseName || `${dbType.label} Connection`;

			this.connectionError = null;
			return true;
		} catch {
			this.connectionError = "Invalid connection string format";
			return false;
		}
	}

	// Build connection string from form data
	buildConnectionString(): string {
		const data = this.formData;

		if (data.type === "sqlite") {
			return `sqlite://${data.databaseName}`;
		}

		const credentials = data.username
			? `${encodeURIComponent(data.username)}${data.password ? `:${encodeURIComponent(data.password)}` : ""}@`
			: "";

		const protocol = this.selectedDbType?.protocol[0] || data.type;
		const port = data.port !== this.selectedDbType?.defaultPort ? `:${data.port}` : "";

		let connectionString = `${protocol}://${credentials}${data.host}${port}/${data.databaseName}`;

		// Add sslmode parameter for PostgreSQL and MySQL
		if (
			(data.type === "postgres" || data.type === "mysql" || data.type === "mariadb") &&
			data.sslMode &&
			data.sslMode !== "disable"
		) {
			const separator = connectionString.includes("?") ? "&" : "?";
			connectionString += `${separator}sslmode=${data.sslMode}`;
		}

		return connectionString;
	}

	// Get connection data for database connection
	getConnectionData() {
		let connString = this.formData.connectionString;
		if (connString) {
			connString = connString.replace("postgresql://", "postgres://");
		} else {
			connString = this.buildConnectionString();
		}

		if (!connString || connString.split(":").length !== 3) {
			connString = this.buildConnectionString();
		}

		return {
			name: this.formData.name,
			type: this.formData.type,
			host: this.formData.host,
			port: this.formData.port,
			databaseName: this.formData.databaseName,
			username: this.formData.username,
			password: this.formData.password,
			sslMode: this.formData.sslMode,
			connectionString: connString,
			sshTunnel: this.formData.sshEnabled
				? {
						enabled: true,
						host: this.formData.sshHost,
						port: this.formData.sshPort,
						username: this.formData.sshUsername,
						authMethod: this.formData.sshAuthMethod,
					}
				: undefined,
			sshPassword: this.formData.sshPassword,
			sshKeyPath: this.formData.sshKeyPath,
			sshKeyPassphrase: this.formData.sshKeyPassphrase,
		};
	}

	// Mark connection as successful
	markSuccess(): void {
		this.connectionSuccess = true;
		this.currentStep = "success";
	}

	// Set connection error
	setError(error: string): void {
		this.connectionError = error;
	}

	// Clear error
	clearError(): void {
		this.connectionError = null;
	}
}

export const connectionWizardStore = new ConnectionWizardStore();
