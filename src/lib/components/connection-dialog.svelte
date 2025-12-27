<script lang="ts">
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
    import { open as openFileDialog } from "@tauri-apps/plugin-dialog";

    let { open = $bindable(false), prefill = undefined }: Props = $props();
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

    const databaseTypes: {
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

            toast.success("Connection string parsed successfully");
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

const handleSubmit = async () => {
    if (
        activeTab === "connection-string" &&
        formData.connectionString.trim()
    ) {
        parseConnectionString(formData.connectionString.trim());
    }

    if (!formData.name.trim()) {
        toast.error("Please enter a connection name");
        return;
    }

    if (formData.type !== "sqlite" && !formData.host.trim()) {
        toast.error("Please enter a host");
        return;
    }

    if (!formData.databaseName.trim()) {
        toast.error("Please enter a database name");
        return;
    }

    if (formData.connectionString) {
        formData.connectionString = formData.connectionString.replace(
            "postgresql://",
            "postgres://",
        );
    } else {
        formData.connectionString = buildConnectionString(formData);
    }

    if (!formData.connectionString || formData.connectionString.split(":").length != 3) {
        formData.connectionString = buildConnectionString(formData);
    }

    // Build connection data with SSH tunnel config
    const connectionData = {
        name: formData.name,
        type: formData.type,
        host: formData.host,
        port: formData.port,
        databaseName: formData.databaseName,
        username: formData.username,
        password: formData.password,
        sslMode: formData.sslMode,
        connectionString: formData.connectionString,
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

    if (isReconnecting && reconnectingConnectionId) {
        await db.reconnectConnection(reconnectingConnectionId, connectionData);
    } else {
        await db.addConnection(connectionData);
    }

    open = false;

    // Reset form
    resetForm();
    activeTab = "connection-string";
};

    const selectSshKeyFile = async () => {
        try {
            const selected = await openFileDialog({
                multiple: false,
                title: "Select SSH Key File",
            });
            if (selected && typeof selected === "string") {
                formData.sshKeyPath = selected;
            }
        } catch (error) {
            console.error("Failed to select file:", error);
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
            <DialogTitle>{isReconnecting ? "Reconnect to Database" : "New Database Connection"}</DialogTitle>
            <DialogDescription>
                {#if isReconnecting}
                    Enter your password to reconnect. Connection details are pre-filled.
                {:else}
                    Configure your database connection settings
                {/if}
            </DialogDescription>
        </DialogHeader>

        <Tabs bind:value={activeTab} class="w-full">
            <TabsList class="grid w-full grid-cols-2">
                <TabsTrigger value="connection-string"
                    >Connection String</TabsTrigger
                >
                <TabsTrigger value="manual">Manual</TabsTrigger>
            </TabsList>

            <TabsContent
                value="connection-string"
                class="flex flex-col gap-4 mt-4"
            >
                <div class="grid gap-2">
                    <Label for="connection-string">Connection String</Label>
                    <Textarea
                        id="connection-string"
                        bind:value={formData.connectionString}
                        placeholder="postgres://user:password@host:port/database"
                        class="font-mono text-sm min-h-[100px]"
                    />
                    <p class="text-xs text-muted-foreground">
                        Paste your connection string. Supported formats:
                        <br />
                        • PostgreSQL:
                        <code class="text-xs"
                            >postgres://user:pass@host:port/db</code
                        >
                        <br />
                        • MySQL:
                        <code class="text-xs"
                            >mysql://user:pass@host:port/db</code
                        >
                        <br />
                        • SQLite:
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
                    >Parse Connection String</Button
                >

                {#if formData.name}
                    <div
                        class="p-3 rounded-lg border bg-muted/50 flex flex-col gap-2"
                    >
                        <p class="text-sm font-medium">
                            Parsed Connection Details:
                        </p>
                        <div class="text-xs space-y-1">
                            <p>
                                <span class="text-muted-foreground">Type:</span>
                                {selectedDbType?.label}
                            </p>
                            <p>
                                <span class="text-muted-foreground">Host:</span>
                                {formData.host}
                            </p>
                            <p>
                                <span class="text-muted-foreground">Port:</span>
                                {formData.port}
                            </p>
                            <p>
                                <span class="text-muted-foreground"
                                    >Database:</span
                                >
                                {formData.databaseName}
                            </p>
                            <p>
                                <span class="text-muted-foreground"
                                    >Username:</span
                                >
                                {formData.username || "(none)"}
                            </p>
                        </div>
                    </div>
                {/if}
            </TabsContent>
            
            <TabsContent value="manual" class="flex flex-col gap-4 mt-4">
                <div class="grid gap-2">
                    <Label for="name">Connection Name</Label>
                    <Input
                        id="name"
                        bind:value={formData.name}
                        placeholder="My Database"
                    />
                </div>

                <div class="grid gap-2">
                    <Label for="type">Database Type</Label>
                    <Select
                        type="single"
                        value={formData.type}
                        onValueChange={handleTypeChange}
                    >
                        <SelectTrigger id="type" class="w-full">
                            {selectedDbType?.label || "Select database type"}
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
                            <Label for="host">Host</Label>
                            <Input
                                id="host"
                                bind:value={formData.host}
                                placeholder="localhost"
                            />
                        </div>
                        <div class="grid gap-2">
                            <Label for="port">Port</Label>
                            <Input
                                id="port"
                                type="number"
                                bind:value={formData.port}
                            />
                        </div>
                    </div>
                {/if}

                <div class="grid gap-2">
                    <Label for="database">Database</Label>
                    <Input
                        id="database"
                        bind:value={formData.databaseName}
                        placeholder={formData.type === "sqlite"
                            ? "/path/to/database.db"
                            : "database_name"}
                    />
                </div>

                {#if formData.type !== "sqlite"}
                    <div class="grid gap-2">
                        <Label for="username">Username</Label>
                        <Input id="username" bind:value={formData.username} />
                    </div>

                    <div class="grid gap-2">
                        <Label for="password">Password</Label>
                        <Input
                            id="password"
                            type="password"
                            bind:value={formData.password}
                            placeholder={isReconnecting ? "Password required to reconnect" : "Enter password"}
                        />
                        {#if isReconnecting}
                            <p class="text-xs text-amber-600 dark:text-amber-500">
                                ⚠ Password is not stored for security. Please enter your password to reconnect.
                            </p>
                        {/if}
                    </div>

                    {#if formData.type === "postgres" || formData.type === "mysql" || formData.type === "mariadb"}
                        <div class="grid gap-2">
                            <Label for="sslmode">SSL Mode</Label>
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

                    <!-- SSH Tunnel Section -->
                    <div class="border-t pt-4 mt-2">
                        <div class="flex items-center gap-3 mb-4">
                            <input
                                type="checkbox"
                                id="ssh-enabled"
                                bind:checked={formData.sshEnabled}
                                class="h-4 w-4 rounded border-gray-300"
                            />
                            <Label for="ssh-enabled" class="cursor-pointer">Connect via SSH Tunnel</Label>
                        </div>

                        {#if formData.sshEnabled}
                            <div class="flex flex-col gap-4">
                                <div class="grid grid-cols-3 gap-2">
                                    <div class="col-span-2 grid gap-2">
                                        <Label for="ssh-host">SSH Host</Label>
                                        <Input
                                            id="ssh-host"
                                            bind:value={formData.sshHost}
                                            placeholder="ssh.example.com"
                                        />
                                    </div>
                                    <div class="grid gap-2">
                                        <Label for="ssh-port">SSH Port</Label>
                                        <Input
                                            id="ssh-port"
                                            type="number"
                                            bind:value={formData.sshPort}
                                        />
                                    </div>
                                </div>

                                <div class="grid gap-2">
                                    <Label for="ssh-username">SSH Username</Label>
                                    <Input
                                        id="ssh-username"
                                        bind:value={formData.sshUsername}
                                        placeholder="username"
                                    />
                                </div>

                                <div class="grid gap-2">
                                    <Label>Authentication Method</Label>
                                    <Select
                                        type="single"
                                        value={formData.sshAuthMethod}
                                        onValueChange={(value) => (formData.sshAuthMethod = value as SSHAuthMethod)}
                                    >
                                        <SelectTrigger class="w-full">
                                            {formData.sshAuthMethod === "password" ? "Password" : "SSH Key"}
                                        </SelectTrigger>
                                        <SelectContent>
                                            <SelectItem value="password">Password</SelectItem>
                                            <SelectItem value="key">SSH Key</SelectItem>
                                        </SelectContent>
                                    </Select>
                                </div>

                                {#if formData.sshAuthMethod === "password"}
                                    <div class="grid gap-2">
                                        <Label for="ssh-password">SSH Password</Label>
                                        <Input
                                            id="ssh-password"
                                            type="password"
                                            bind:value={formData.sshPassword}
                                            placeholder="SSH password"
                                        />
                                    </div>
                                {:else}
                                    <div class="grid gap-2">
                                        <Label for="ssh-key-path">SSH Key File</Label>
                                        <div class="flex gap-2">
                                            <Input
                                                id="ssh-key-path"
                                                bind:value={formData.sshKeyPath}
                                                placeholder="~/.ssh/id_rsa"
                                                class="flex-1"
                                            />
                                            <Button variant="outline" type="button" onclick={selectSshKeyFile}>
                                                Browse
                                            </Button>
                                        </div>
                                    </div>
                                    <div class="grid gap-2">
                                        <Label for="ssh-key-passphrase">Key Passphrase (if encrypted)</Label>
                                        <Input
                                            id="ssh-key-passphrase"
                                            type="password"
                                            bind:value={formData.sshKeyPassphrase}
                                            placeholder="Optional"
                                        />
                                    </div>
                                {/if}

                                {#if isReconnecting && formData.sshEnabled}
                                    <p class="text-xs text-amber-600 dark:text-amber-500">
                                        ⚠ SSH credentials are not stored for security. Please enter them to reconnect.
                                    </p>
                                {/if}
                            </div>
                        {/if}
                    </div>
                {/if}
            </TabsContent>
        </Tabs>

        <DialogFooter>
            <Button variant="outline" onclick={() => (open = false)}
                >Cancel</Button
            >
            <Button onclick={handleSubmit}>{isReconnecting ? "Reconnect" : "Add Connection"}</Button>
        </DialogFooter>
    </DialogContent>
</Dialog>
