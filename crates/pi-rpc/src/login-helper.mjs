// Provider sign-in for GUI hosts. JSONL over stdio; never prints secrets.
// Usage: PI_CODING_AGENT_DIR=<home> node login-helper.mjs <pi-coding-agent package dir>
// In:  {"type":"list"} | {"type":"login","provider","authType":"oauth"|"api_key"} | {"type":"logout","provider"}
//      {"type":"response","id","value"} | {"type":"cancel_prompt","id"} | {"type":"cancel"}
// Out: {"type":"providers",providers} | {"type":"prompt",id,prompt} | {"type":"prompt_dismissed",id}
//      {"type":"event",event} | {"type":"done",provider?} | {"type":"error",message}
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const pkg = process.argv[2];
const { ModelRuntime, SettingsManager, getAgentDir } = await import(pathToFileURL(join(pkg, "dist", "index.js")).href);

const out = (record) => process.stdout.write(`${JSON.stringify(record)}\n`);
const agentDir = getAgentDir();
const runtime = await ModelRuntime.create({ authPath: join(agentDir, "auth.json"), modelsPath: join(agentDir, "models.json") });
const settings = SettingsManager.create(process.cwd(), agentDir);
const pending = new Map();
let abort = new AbortController();
let seq = 0;

function providers() {
	return runtime
		.getProviders()
		.map((p) => {
			const status = runtime.getProviderAuthStatus(p.id) ?? {};
			return {
				id: p.id,
				name: p.name ?? p.id,
				configured: Boolean(status.configured),
				source: status.source ?? null,
				oauth: p.auth?.oauth
					? { label: p.auth.oauth.loginLabel ?? p.auth.oauth.name ?? "Sign in with an account", subscription: Boolean(p.auth.oauth.isSubscription) }
					: null,
				apiKey: p.auth?.apiKey ? { interactive: Boolean(p.auth.apiKey.login) } : null,
			};
		})
		.filter((p) => p.oauth || p.apiKey)
		.sort((a, b) => a.name.localeCompare(b.name));
}

async function login(provider, authType) {
	abort = new AbortController();
	try {
		await runtime.login(
			provider,
			authType,
			{
				signal: abort.signal,
				notify: (event) => out({ type: "event", event }),
				prompt: (p) =>
					new Promise((resolve, reject) => {
						const id = ++seq;
						const { signal, ...prompt } = p;
						pending.set(id, { resolve, reject });
						out({ type: "prompt", id, prompt });
						signal?.addEventListener(
							"abort",
							() => {
								if (pending.delete(id)) {
									out({ type: "prompt_dismissed", id });
									reject(new Error("Login cancelled"));
								}
							},
							{ once: true },
						);
					}),
			},
			{ getDeviceId: () => settings.getOrCreateDeviceId() },
		);
		out({ type: "done", provider });
	} catch (error) {
		out({ type: "error", message: String(error?.message ?? error) });
	}
}

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
	buffer += chunk;
	let newline;
	while ((newline = buffer.indexOf("\n")) >= 0) {
		const line = buffer.slice(0, newline).replace(/\r$/, "");
		buffer = buffer.slice(newline + 1);
		if (!line.trim()) continue;
		let message;
		try {
			message = JSON.parse(line);
		} catch {
			out({ type: "error", message: "malformed request" });
			continue;
		}
		if (message.type === "list") out({ type: "providers", providers: providers() });
		else if (message.type === "login") void login(message.provider, message.authType ?? "oauth");
		else if (message.type === "logout")
			runtime.logout(message.provider).then(
				() => out({ type: "done", provider: message.provider }),
				(error) => out({ type: "error", message: String(error?.message ?? error) }),
			);
		else if (message.type === "response") {
			pending.get(message.id)?.resolve(message.value);
			pending.delete(message.id);
		} else if (message.type === "cancel_prompt") {
			pending.get(message.id)?.reject(new Error("Login cancelled"));
			pending.delete(message.id);
		} else if (message.type === "cancel") abort.abort();
	}
});
process.stdin.on("end", () => process.exit(0));
out({ type: "ready" });
