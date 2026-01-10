<script lang="ts">
    import { m } from "$lib/paraglide/messages.js";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import {
        Dialog,
        DialogContent,
        DialogDescription,
        DialogFooter,
        DialogHeader,
        DialogTitle,
    } from "$lib/components/ui/dialog";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import { Label } from "$lib/components/ui/label";
    import { Textarea } from "$lib/components/ui/textarea";
    import {
        Select,
        SelectContent,
        SelectItem,
        SelectTrigger,
    } from "$lib/components/ui/select";
    import {
        Tabs,
        TabsContent,
        TabsList,
        TabsTrigger,
    } from "$lib/components/ui/tabs";
    import type { DatabaseType, SSHAuthMethod } from "$lib/types";
    import { toast } from "svelte-sonner";
    import CopyIcon from "@lucide/svelte/icons/copy";
    import { SshTunnelConfig } from "$lib/components/connection-dialog/index.js";
    import { getFeatures } from "$lib/features";

    let { open = $bindable(false), prefill = undefined }: Props = $props();
    const features = getFeatures();
    const db = useDatabase();

    let formData = $state({
        name: "",
        type: "postgres" as DatabaseType,
        host: "localhost",
        port: 5432,
        databaseName: "",
        username: "",
        password: "",
        sslMode: "disable",
        connectionString: "",
        // SSH Tunnel fields
        sshEnabled: false,
        sshHost: "",
        sshPort: 22,
        sshUsername: "",
        sshAuthMethod: "password" as SSHAuthMethod,
        sshPassword: "",
        sshKeyPath: "",
        sshKeyPassphrase: "",
    });

    let activeTab = $state<"connection-string" | "manual">("connection-string");
    let isReconnecting = $state(false);
    let reconnectingConnectionId = $state<string | null>(null);
    let isConnecting = $state(false);
    let isTesting = $state(false);
    let connectionError = $state<string | null>(null);

    const resetForm = () => {
        formData = {
            name: "",
            type: "postgres",
            host: "localhost",
            port: 5432,
            databaseName: "",
            username: "",
            password: "",
            sslMode: "disable",
            connectionString: "",
            // SSH Tunnel fields
            sshEnabled: false,
            sshHost: "",
            sshPort: 22,
            sshUsername: "",
            sshAuthMethod: "password",
            sshPassword: "",
            sshKeyPath: "",
            sshKeyPassphrase: "",
        };
    };

    // Initialize form with prefilled data when dialog opens
    $effect(() => {
        if (open) {
            connectionError = null;
            if (prefill) {
                formData = {
                    name: prefill.name || "",
                    type: (prefill.type as DatabaseType) || "postgres",
                    host: prefill.host || "localhost",
                    port: prefill.port || 5432,
                    databaseName: prefill.databaseName || "",
                    username: prefill.username || "",
                    password: "", // Always empty for security - user must re-enter
                    sslMode: prefill.sslMode || "disable",
                    connectionString: prefill.connectionString || "",
                    // SSH Tunnel prefill (credentials always empty for security)
                    sshEnabled: prefill.sshTunnel?.enabled || false,
                    sshHost: prefill.sshTunnel?.host || "",
                    sshPort: prefill.sshTunnel?.port || 22,
                    sshUsername: prefill.sshTunnel?.username || "",
                    sshAuthMethod: prefill.sshTunnel?.authMethod || "password",
                    sshPassword: "",
                    sshKeyPath: "",
                    sshKeyPassphrase: "",
                };
                activeTab = prefill.connectionString ? "connection-string" : "manual";
                isReconnecting = true;
                reconnectingConnectionId = prefill.id || null;
            } else {
                resetForm();
                activeTab = "connection-string";
                isReconnecting = false;
                reconnectingConnectionId = null;
            }
        }
    });

    const sslModes = ["disable", "allow", "prefer", "require"];

    const allDatabaseTypes: {
        value: DatabaseType;
        label: string;
        defaultPort: number;
        protocol: string[];
    }[] = [
        {
            value: "postgres",
            label: "PostgreSQL",
            defaultPort: 5432,
            protocol: ["postgres", "postgresql"],
        },
        {
            value: "mysql",
            label: "MySQL",
            defaultPort: 3306,
            protocol: ["mysql"],
        },
        {
            value: "mariadb",
            label: "MariaDB",
            defaultPort: 3306,
            protocol: ["mariadb"],
        },
        {
            value: "sqlite",
            label: "SQLite",
            defaultPort: 0,
            protocol: ["sqlite"],
        },
        {
            value: "mongodb",
            label: "MongoDB",
            defaultPort: 27017,
            protocol: ["mongodb", "mongodb+srv"],
        },
        {
            value: "mssql",
            label: "Microsoft SQL Server",
            defaultPort: 1433,
            protocol: ["mssql", "sqlserver"],
        },
    ];

    // Filter database types based on feature flags
    const databaseTypes = features.mssqlSupport
        ? allDatabaseTypes
        : allDatabaseTypes.filter(t => t.value !== "mssql");

    const selectedDbType = $derived(
        databaseTypes.find((t) => t.value === formData.type),
    );

    const handleTypeChange = (value: string) => {
        formData.type = value as DatabaseType;
        const dbType = databaseTypes.find((t) => t.value === value);
        if (dbType) {
            formData.port = dbType.defaultPort;
        }
    };

    const parseConnectionString = (connStr: string) => {
        try {
            // Handle special cases
            if (
                connStr.startsWith("sqlite://") ||
                connStr.startsWith("sqlite:")
            ) {
                const dbPath = connStr
                    .replace(/^sqlite:\/\//, "")
                    .replace(/^sqlite:/, "");
                formData.type = "sqlite";
                formData.databaseName = dbPath;
                formData.name = `SQLite - ${dbPath.split("/").pop() || "database"}`;
                return;
            }
            connStr = connStr.replace("postgresql://", "postgres://");

            // Parse as URL
            const url = new URL(connStr);

            // Detect database type from protocol
            const protocol = url.protocol.replace(":", "");
            const dbType = databaseTypes.find((t) =>
                t.protocol.includes(protocol),
            );

            if (!dbType) {
                toast.error(`Unsupported database type: ${protocol}`);
                return;
            }

            formData.type = dbType.value;
            formData.host = url.hostname;
            formData.port = url.port ? parseInt(url.port) : dbType.defaultPort;
            formData.databaseName = url.pathname.replace(/^\//, "");
            formData.username = url.username
                ? decodeURIComponent(url.username)
                : "";
            formData.password = url.password
                ? decodeURIComponent(url.password)
                : "";

            // Parse SSL mode from query parameters
            const sslModeParam = url.searchParams.get("sslmode");
            if (sslModeParam && sslModes.includes(sslModeParam)) {
                formData.sslMode = sslModeParam;
            }

            // Generate a name from the database
            formData.name =
                formData.databaseName || `${dbType.label} Connection`;
        } catch (error) {
            toast.error("Invalid connection string format");
            console.error("Parse error:", error);
        }
    };

    const buildConnectionString = (data: typeof formData) => {
        if (data.type === "sqlite") {
            return `sqlite://${data.databaseName}`;
        }

        const credentials = data.username
            ? `${encodeURIComponent(data.username)}${data.password ? `:${encodeURIComponent(data.password)}` : ""}@`
            : "";

        const protocol = selectedDbType?.protocol[0] || data.type;
        const port =
            data.port !== selectedDbType?.defaultPort ? `:${data.port}` : "";

        let connectionString = `${protocol}://${credentials}${data.host}${port}/${data.databaseName}`;

        // Add sslmode parameter for PostgreSQL and MySQL
        if ((data.type === "postgres" || data.type === "mysql" || data.type === "mariadb") && data.sslMode && data.sslMode !== "disable") {
            const separator = connectionString.includes("?") ? "&" : "?";
            connectionString += `${separator}sslmode=${data.sslMode}`;
        }

        return connectionString;
    };

    const getConnectionData = () => {
        if (
            activeTab === "connection-string" &&
            formData.connectionString.trim()
        ) {
            parseConnectionString(formData.connectionString.trim());
        }

        let connString = formData.connectionString;
        if (connString) {
            connString = connString.replace("postgresql://", "postgres://");
        } else {
            connString = buildConnectionString(formData);
        }

        if (!connString || connString.split(":").length != 3) {
            connString = buildConnectionString(formData);
        }

        return {
            name: formData.name,
            type: formData.type,
            host: formData.host,
            port: formData.port,
            databaseName: formData.databaseName,
            username: formData.username,
            password: formData.password,
            sslMode: formData.sslMode,
            connectionString: connString,
            sshTunnel: formData.sshEnabled ? {
                enabled: true,
                host: formData.sshHost,
                port: formData.sshPort,
                username: formData.sshUsername,
                authMethod: formData.sshAuthMethod,
            } : undefined,
            sshPassword: formData.sshPassword,
            sshKeyPath: formData.sshKeyPath,
            sshKeyPassphrase: formData.sshKeyPassphrase,
        };
    };

    const extractErrorMessage = (error: unknown): string => {
        if (error instanceof Error) {
            return error.message;
        }
        if (typeof error === "string") {
            // Extract the meaningful part from database error strings
            const match = error.match(/error returned from database: (.+)/);
            if (match) {
                return match[1];
            }
            return error;
        }
        // Handle Tauri error objects with message/code properties
        if (typeof error === "object" && error !== null) {
            const errObj = error as Record<string, unknown>;
            if (typeof errObj.message === "string") {
                return errObj.code ? `${errObj.code}: ${errObj.message}` : errObj.message;
            }
            // Log for debugging
            console.error("Unknown error object:", error);
        }
        return "An unknown error occurred";
    };

    const handleTestConnection = async () => {
        connectionError = null;

        if (!formData.name.trim()) {
            connectionError = m.connection_dialog_error_name_required();
            return;
        }

        if (formData.type !== "sqlite" && !formData.host.trim()) {
            connectionError = m.connection_dialog_error_host_required();
            return;
        }

        if (!formData.databaseName.trim()) {
            connectionError = m.connection_dialog_error_database_required();
            return;
        }

        isTesting = true;
        try {
            const connectionData = getConnectionData();
            await db.connections.test(connectionData);
            toast.success("Connection successful");
        } catch (error) {
            connectionError = extractErrorMessage(error);
        } finally {
            isTesting = false;
        }
    };

    const handleSubmit = async () => {
        connectionError = null;

        if (!formData.name.trim()) {
            connectionError = m.connection_dialog_error_name_required();
            return;
        }

        if (formData.type !== "sqlite" && !formData.host.trim()) {
            connectionError = m.connection_dialog_error_host_required();
            return;
        }

        if (!formData.databaseName.trim()) {
            connectionError = m.connection_dialog_error_database_required();
            return;
        }

        isConnecting = true;
        try {
            const connectionData = getConnectionData();

            if (isReconnecting && reconnectingConnectionId) {
                await db.connections.reconnect(reconnectingConnectionId, connectionData);
            } else {
                await db.connections.add(connectionData);
            }

            // Show appropriate toast based on whether database has tables
            if (db.state.activeSchema.length === 0) {
                toast.warning("Connected, but database is empty - no tables found");
            } else {
                toast.success("Connected successfully");
            }
            open = false;
            resetForm();
            activeTab = "connection-string";
        } catch (error) {
            connectionError = extractErrorMessage(error);
        } finally {
            isConnecting = false;
        }
    };

    type Props = {
        open?: boolean;
        prefill?: {
            id?: string;
            name?: string;
            type?: DatabaseType;
            host?: string;
            port?: number;
            databaseName?: string;
            username?: string;
            sslMode?: string;
            connectionString?: string;
            sshTunnel?: {
                enabled: boolean;
                host: string;
                port: number;
                username: string;
                authMethod: SSHAuthMethod;
            };
        };
    };
</script>

<Dialog bind:open>
    <DialogContent class="max-w-md max-h-[90vh] overflow-y-auto">
        <DialogHeader>
            <DialogTitle>{isReconnecting ? m.connection_dialog_title_reconnect() : m.connection_dialog_title_new()}</DialogTitle>
            <DialogDescription>
                {#if isReconnecting}
                    {m.connection_dialog_description_reconnect()}
                {:else}
                    {m.connection_dialog_description_new()}
                {/if}
            </DialogDescription>
        </DialogHeader>

        <Tabs bind:value={activeTab} class="w-full">
            <TabsList class="grid w-full grid-cols-2">
                <TabsTrigger value="connection-string"
                    >{m.connection_dialog_tab_connection_string()}</TabsTrigger
                >
                <TabsTrigger value="manual">{m.connection_dialog_tab_manual()}</TabsTrigger>
            </TabsList>

            <TabsContent
                value="connection-string"
                class="flex flex-col gap-4 mt-4"
            >
                <div class="grid gap-2">
                    <Label for="connection-string">{m.connection_dialog_label_connection_string()}</Label>
                    <Textarea
                        id="connection-string"
                        bind:value={formData.connectionString}
                        placeholder={m.connection_dialog_placeholder_connection_string()}
                        class="font-mono text-sm min-h-[100px]"
                    />
                    <p class="text-xs text-muted-foreground">
                        {m.connection_dialog_help_formats()}
                        <br />
                        • {m.connection_dialog_help_postgresql()}
                        <code class="text-xs"
                            >postgres://user:pass@host:port/db</code
                        >
                        <br />
                        • {m.connection_dialog_help_mysql()}
                        <code class="text-xs"
                            >mysql://user:pass@host:port/db</code
                        >
                        <br />
                        • {m.connection_dialog_help_sqlite()}
                        <code class="text-xs"
                            >sqlite:///path/to/database.db</code
                        >
                    </p>
                </div>

                <Button
                    type="button"
                    variant="outline"
                    class="w-full"
                    onclick={() =>
                        parseConnectionString(formData.connectionString)}
                    disabled={!formData.connectionString.trim()}
                    >{m.connection_dialog_button_parse()}</Button
                >

                {#if formData.name}
                    <div
                        class="p-3 rounded-lg border bg-muted/50 flex flex-col gap-2"
                    >
                        <p class="text-sm font-medium">
                            {m.connection_dialog_parsed_title()}
                        </p>
                        <div class="text-xs space-y-1">
                            <p>
                                <span class="text-muted-foreground">{m.connection_dialog_parsed_type()}</span>
                                {selectedDbType?.label}
                            </p>
                            <p>
                                <span class="text-muted-foreground">{m.connection_dialog_parsed_host()}</span>
                                {formData.host}
                            </p>
                            <p>
                                <span class="text-muted-foreground">{m.connection_dialog_parsed_port()}</span>
                                {formData.port}
                            </p>
                            <p>
                                <span class="text-muted-foreground"
                                    >{m.connection_dialog_parsed_database()}</span
                                >
                                {formData.databaseName}
                            </p>
                            <p>
                                <span class="text-muted-foreground"
                                    >{m.connection_dialog_parsed_username()}</span
                                >
                                {formData.username || m.connection_dialog_parsed_none()}
                            </p>
                        </div>
                    </div>
                {/if}
            </TabsContent>
            
            <TabsContent value="manual" class="flex flex-col gap-4 mt-4">
                <div class="grid gap-2">
                    <Label for="name">{m.connection_dialog_label_connection_name()}</Label>
                    <Input
                        id="name"
                        bind:value={formData.name}
                        placeholder={m.connection_dialog_placeholder_connection_name()}
                    />
                </div>

                <div class="grid gap-2">
                    <Label for="type">{m.connection_dialog_label_database_type()}</Label>
                    <Select
                        type="single"
                        value={formData.type}
                        onValueChange={handleTypeChange}
                    >
                        <SelectTrigger id="type" class="w-full">
                            {selectedDbType?.label || m.connection_dialog_placeholder_select_db_type()}
                        </SelectTrigger>
                        <SelectContent>
                            {#each databaseTypes as dbType}
                                <SelectItem value={dbType.value}
                                    >{dbType.label}</SelectItem
                                >
                            {/each}
                        </SelectContent>
                    </Select>
                </div>

                {#if formData.type !== "sqlite"}
                    <div class="grid grid-cols-3 gap-2">
                        <div class="col-span-2 grid gap-2">
                            <Label for="host">{m.connection_dialog_label_host()}</Label>
                            <Input
                                id="host"
                                bind:value={formData.host}
                                placeholder={m.connection_dialog_placeholder_host()}
                            />
                        </div>
                        <div class="grid gap-2">
                            <Label for="port">{m.connection_dialog_label_port()}</Label>
                            <Input
                                id="port"
                                type="number"
                                bind:value={formData.port}
                            />
                        </div>
                    </div>
                {/if}

                <div class="grid gap-2">
                    <Label for="database">{m.connection_dialog_label_database()}</Label>
                    <Input
                        id="database"
                        bind:value={formData.databaseName}
                        placeholder={formData.type === "sqlite"
                            ? m.connection_dialog_placeholder_database_path()
                            : m.connection_dialog_placeholder_database_name()}
                    />
                </div>

                {#if formData.type !== "sqlite"}
                    <div class="grid gap-2">
                        <Label for="username">{m.connection_dialog_label_username()}</Label>
                        <Input id="username" bind:value={formData.username} />
                    </div>

                    <div class="grid gap-2">
                        <Label for="password">{m.connection_dialog_label_password()}</Label>
                        <Input
                            id="password"
                            type="password"
                            bind:value={formData.password}
                            placeholder={isReconnecting ? m.connection_dialog_placeholder_password_reconnect() : m.connection_dialog_placeholder_password()}
                        />
                        {#if isReconnecting}
                            <p class="text-xs text-amber-600 dark:text-amber-500">
                                ⚠ {m.connection_dialog_warning_password()}
                            </p>
                        {/if}
                    </div>

                    {#if formData.type === "postgres" || formData.type === "mysql" || formData.type === "mariadb"}
                        <div class="grid gap-2">
                            <Label for="sslmode">{m.connection_dialog_label_ssl_mode()}</Label>
                            <Select
                                type="single"
                                value={formData.sslMode}
                                onValueChange={(value) => (formData.sslMode = value)}
                            >
                                <SelectTrigger id="sslmode" class="w-full">
                                    {formData.sslMode}
                                </SelectTrigger>
                                <SelectContent>
                                    {#each sslModes as mode}
                                        <SelectItem value={mode}>{mode}</SelectItem>
                                    {/each}
                                </SelectContent>
                            </Select>
                        </div>
                    {/if}

                    <!-- SSH Tunnel Section (desktop only) -->
                    {#if features.sshTunnels}
                        <SshTunnelConfig
                            enabled={formData.sshEnabled}
                            host={formData.sshHost}
                            port={formData.sshPort}
                            username={formData.sshUsername}
                            authMethod={formData.sshAuthMethod}
                            password={formData.sshPassword}
                            keyPath={formData.sshKeyPath}
                            keyPassphrase={formData.sshKeyPassphrase}
                            {isReconnecting}
                            onEnabledChange={(v) => formData.sshEnabled = v}
                            onHostChange={(v) => formData.sshHost = v}
                            onPortChange={(v) => formData.sshPort = v}
                            onUsernameChange={(v) => formData.sshUsername = v}
                            onAuthMethodChange={(v) => formData.sshAuthMethod = v}
                            onPasswordChange={(v) => formData.sshPassword = v}
                            onKeyPathChange={(v) => formData.sshKeyPath = v}
                            onKeyPassphraseChange={(v) => formData.sshKeyPassphrase = v}
                        />
                    {/if}
                {/if}
            </TabsContent>
        </Tabs>

        {#if connectionError}
            <div class="flex items-start gap-2 p-3 rounded-lg border border-destructive/50 bg-destructive/10 text-destructive text-sm">
                <span class="flex-1">{connectionError}</span>
                <Button
                    variant="ghost"
                    size="icon"
                    class="shrink-0 size-6 text-destructive/70 hover:text-destructive hover:bg-destructive/20"
                    onclick={async () => {
                        try {
                            await navigator.clipboard.writeText(connectionError ?? '');
                            toast.success(m.query_error_copied());
                        } catch {
                            toast.error(m.query_copy_failed());
                        }
                    }}
                >
                    <CopyIcon class="size-3.5" />
                </Button>
            </div>
        {/if}

        <DialogFooter class="flex-col sm:flex-row gap-2">
            <Button
                variant="outline"
                onclick={handleTestConnection}
                disabled={isConnecting || isTesting}
                class="sm:me-auto"
            >
                {#if isTesting}
                    {m.connection_dialog_button_testing()}
                {:else}
                    {m.connection_dialog_button_test()}
                {/if}
            </Button>
            <div class="flex gap-2">
                <Button variant="outline" onclick={() => (open = false)} disabled={isConnecting || isTesting}>
                    {m.connection_dialog_button_cancel()}
                </Button>
                <Button onclick={handleSubmit} disabled={isConnecting || isTesting}>
                    {#if isConnecting}
                        {m.connection_dialog_button_connecting()}
                    {:else if isReconnecting}
                        {m.connection_dialog_button_reconnect()}
                    {:else}
                        {m.connection_dialog_button_add()}
                    {/if}
                </Button>
            </div>
        </DialogFooter>
    </DialogContent>
</Dialog>
