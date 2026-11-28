// A typed client for the agent's /v1 API (docs/http-api.md).

import type {
	AgentStatus,
	CheckDefinition,
	CheckView,
	DiscoveryFilter,
	NodeInfo,
	RegistrationReceipt,
	RegistryEvent,
	Scope,
	ServiceInstance,
	ServiceRegistration,
	ServiceSummary
} from './types';

/** A non-2xx response, or a failure to reach the agent (`status` 0, code `unavailable`). */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string,
		message: string
	) {
		super(message);
		this.name = 'ApiError';
	}
}

export interface ClientOptions {
	fetch?: typeof fetch;
	/** The API token, read on every request. */
	token?: () => string | null;
	/** Called on a 401, so the app can ask for a token. */
	onUnauthorized?: () => void;
}

type Query = [string, string][];

export function scopeQuery(scope: Scope = {}): Query {
	const q: Query = [];
	if (scope.namespace) q.push(['namespace', scope.namespace]);
	if (scope.region) q.push(['region', scope.region]);
	return q;
}

export function discoveryQuery(f: DiscoveryFilter = {}): Query {
	const q = scopeQuery(f);
	if (f.health && f.health !== 'any') q.push(['health', f.health]);
	for (const t of f.tags ?? []) q.push(['tag', t]);
	for (const m of f.meta ?? []) q.push(['meta', m]);
	if (f.node) q.push(['node', f.node]);
	return q;
}

function withQuery(path: string, query: Query): string {
	return query.length ? `${path}?${new URLSearchParams(query)}` : path;
}

const seg = encodeURIComponent;

/** Parse a stream of newline-delimited JSON, yielding one value per line. */
export async function* ndjson<T>(body: ReadableStream<Uint8Array>): AsyncGenerator<T> {
	const reader = body.getReader();
	const decoder = new TextDecoder();
	let buffered = '';
	try {
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			buffered += decoder.decode(value, { stream: true });
			let newline: number;
			while ((newline = buffered.indexOf('\n')) >= 0) {
				const line = buffered.slice(0, newline).trim();
				buffered = buffered.slice(newline + 1);
				if (line) yield JSON.parse(line) as T;
			}
		}
		buffered += decoder.decode();
		if (buffered.trim()) yield JSON.parse(buffered) as T;
	} finally {
		reader.releaseLock();
	}
}

export class Client {
	readonly #fetch: typeof fetch;
	readonly #token: () => string | null;
	readonly #onUnauthorized: () => void;

	constructor(options: ClientOptions = {}) {
		this.#fetch = options.fetch ?? ((...args) => fetch(...args));
		this.#token = options.token ?? (() => null);
		this.#onUnauthorized = options.onUnauthorized ?? (() => {});
	}

	async #send(
		method: string,
		path: string,
		body?: unknown,
		signal?: AbortSignal
	): Promise<Response> {
		const headers: Record<string, string> = { accept: 'application/json' };
		const token = this.#token();
		if (token) headers.authorization = `Bearer ${token}`;
		if (body !== undefined) headers['content-type'] = 'application/json';
		let res: Response;
		try {
			res = await this.#fetch(path, {
				method,
				headers,
				body: body === undefined ? undefined : JSON.stringify(body),
				signal
			});
		} catch (e) {
			if (e instanceof DOMException && e.name === 'AbortError') throw e;
			throw new ApiError(0, 'unavailable', 'Cannot reach the agent. Is it running?');
		}
		if (res.ok) return res;
		let code = 'error';
		let message = `${res.status} ${res.statusText}`.trim();
		try {
			const err = (await res.json()) as { error?: { code?: string; message?: string } };
			code = err.error?.code ?? code;
			message = err.error?.message ?? message;
		} catch {
			// Not an API error body (a proxy, for example): keep the status line.
		}
		if (res.status === 401) this.#onUnauthorized();
		throw new ApiError(res.status, code, message);
	}

	async #json<T>(method: string, path: string, body?: unknown): Promise<T> {
		return (await (await this.#send(method, path, body)).json()) as T;
	}

	status = () => this.#json<AgentStatus>('GET', '/v1/status');
	nodes = (scope?: Scope) =>
		this.#json<NodeInfo[]>('GET', withQuery('/v1/nodes', scopeQuery(scope)));
	services = (scope?: Scope) =>
		this.#json<ServiceSummary[]>('GET', withQuery('/v1/services', scopeQuery(scope)));
	discover = (name: string, filter?: DiscoveryFilter) =>
		this.#json<ServiceInstance[]>(
			'GET',
			withQuery(`/v1/discover/${seg(name)}`, discoveryQuery(filter))
		);

	localServices = () => this.#json<ServiceInstance[]>('GET', '/v1/agent/services');
	localService = (id: string) =>
		this.#json<ServiceInstance>('GET', `/v1/agent/services/${seg(id)}`);
	definition = (id: string) =>
		this.#json<ServiceRegistration>('GET', `/v1/agent/services/${seg(id)}/definition`);
	register = (reg: ServiceRegistration) =>
		this.#json<RegistrationReceipt>('POST', '/v1/agent/services', reg);
	deregister = async (id: string) => {
		await this.#send('DELETE', `/v1/agent/services/${seg(id)}`);
	};

	localChecks = () => this.#json<CheckView[]>('GET', '/v1/agent/checks');
	addCheck = (serviceId: string, check: CheckDefinition) =>
		this.#json<{ check_id: string }>('POST', '/v1/agent/checks', {
			service_id: serviceId,
			...check
		});
	removeCheck = async (id: string) => {
		await this.#send('DELETE', `/v1/agent/checks/${seg(id)}`);
	};
	/** Report a TTL check. */
	report = async (id: string, verb: 'pass' | 'warn' | 'fail', output = '') => {
		await this.#send('PUT', `/v1/agent/checks/${seg(id)}/${verb}`, output ? { output } : undefined);
	};

	/** The current result of a discovery query, then a new one on every change, until `signal` aborts. */
	async *watch(
		name: string,
		filter: DiscoveryFilter,
		signal: AbortSignal
	): AsyncGenerator<RegistryEvent> {
		const path = withQuery(`/v1/watch/${seg(name)}`, discoveryQuery(filter));
		const res = await this.#send('GET', path, undefined, signal);
		if (!res.body) throw new ApiError(0, 'unavailable', 'The agent sent no watch stream');
		yield* ndjson<RegistryEvent>(res.body);
	}
}
