import { describe, expect, it, vi } from 'vitest';
import { ApiError, Client, discoveryQuery, ndjson, scopeQuery } from './api';

function respond(status: number, body?: unknown, statusText = '') {
	return new Response(body === undefined ? null : JSON.stringify(body), {
		status,
		statusText,
		headers: { 'content-type': 'application/json' }
	});
}

function client(
	handler: (url: string, init: RequestInit) => Response | Promise<Response>,
	token = ''
) {
	const fetch = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) =>
		handler(String(url), init ?? {})
	);
	const onUnauthorized = vi.fn();
	const c = new Client({
		fetch: fetch as typeof globalThis.fetch,
		token: () => token || null,
		onUnauthorized
	});
	return { c, fetch, onUnauthorized };
}

function stream(chunks: string[]): ReadableStream<Uint8Array> {
	const encoder = new TextEncoder();
	return new ReadableStream({
		start(controller) {
			for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
			controller.close();
		}
	});
}

describe('query strings', () => {
	it('skips empty scope and any-health', () => {
		expect(scopeQuery({})).toEqual([]);
		expect(scopeQuery({ namespace: 'prod', region: '' })).toEqual([['namespace', 'prod']]);
		expect(discoveryQuery({ health: 'any' })).toEqual([]);
	});

	it('repeats tags and metadata', () => {
		expect(
			discoveryQuery({
				region: 'us',
				health: 'passing',
				tags: ['a', 'b'],
				meta: ['shard:4'],
				node: 'n1'
			})
		).toEqual([
			['region', 'us'],
			['health', 'passing'],
			['tag', 'a'],
			['tag', 'b'],
			['meta', 'shard:4'],
			['node', 'n1']
		]);
	});
});

describe('Client', () => {
	it('sends the token and JSON bodies', async () => {
		const { c, fetch } = client(() => respond(201, { service_id: 'web' }), 's3cret');
		await c.register({ name: 'web', port: 80 });
		const [url, init] = fetch.mock.calls[0];
		expect(url).toBe('/v1/agent/services');
		expect(init?.method).toBe('POST');
		expect(init?.body).toBe('{"name":"web","port":80}');
		const headers = init?.headers as Record<string, string>;
		expect(headers.authorization).toBe('Bearer s3cret');
		expect(headers['content-type']).toBe('application/json');
	});

	it('encodes path segments and queries', async () => {
		const { c, fetch } = client(() => respond(200, []));
		await c.discover('web', { tags: ['v 2'], health: 'not-critical' });
		await c.report('service:web', 'pass');
		await c.services({ namespace: 'prod' });
		expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
			'/v1/discover/web?health=not-critical&tag=v+2',
			'/v1/agent/checks/service%3Aweb/pass',
			'/v1/services?namespace=prod'
		]);
	});

	it('sends report output only when given', async () => {
		const { c, fetch } = client(() => new Response(null, { status: 204 }));
		await c.report('beat', 'fail', 'disk full');
		await c.report('beat', 'warn');
		expect(fetch.mock.calls[0][1]?.body).toBe('{"output":"disk full"}');
		expect(fetch.mock.calls[1][1]?.body).toBeUndefined();
	});

	it('turns error bodies into ApiError', async () => {
		const { c } = client(() =>
			respond(404, { error: { code: 'not_found', message: "service 'x' not found" } })
		);
		const err = await c.definition('x').catch((e) => e);
		expect(err).toBeInstanceOf(ApiError);
		expect(err).toMatchObject({ status: 404, code: 'not_found', message: "service 'x' not found" });
	});

	it('keeps the status line when the body is not an API error', async () => {
		const { c } = client(
			() => new Response('<html>bad gateway</html>', { status: 502, statusText: 'Bad Gateway' })
		);
		await expect(c.status()).rejects.toMatchObject({
			status: 502,
			code: 'error',
			message: '502 Bad Gateway'
		});
	});

	it('reports 401 so the app can ask for a token', async () => {
		const { c, onUnauthorized } = client(() =>
			respond(401, { error: { code: 'unauthorized', message: 'missing or wrong API token' } })
		);
		await expect(c.localServices()).rejects.toMatchObject({ status: 401 });
		expect(onUnauthorized).toHaveBeenCalledOnce();
	});

	it('reports an unreachable agent', async () => {
		const { c } = client(() => {
			throw new TypeError('Failed to fetch');
		});
		await expect(c.status()).rejects.toMatchObject({ status: 0, code: 'unavailable' });
	});

	it('streams watch events', async () => {
		const events = [
			{ index: 1, instances: [] },
			{ index: 2, instances: [{ service_id: 'web' }] }
		];
		const { c, fetch } = client(
			() =>
				new Response(
					stream([
						JSON.stringify(events[0]) + '\n' + JSON.stringify(events[1]).slice(0, 5),
						JSON.stringify(events[1]).slice(5) + '\n'
					])
				)
		);
		const got = [];
		for await (const e of c.watch('web', { health: 'passing' }, new AbortController().signal))
			got.push(e);
		expect(got).toEqual(events);
		expect(String(fetch.mock.calls[0][0])).toBe('/v1/watch/web?health=passing');
	});
});

describe('ndjson', () => {
	it('splits lines across chunks and skips blank lines', async () => {
		const out = [];
		for await (const v of ndjson(stream(['{"a":', '1}\n\n{"a":2}\n{"a"', ':3}']))) out.push(v);
		expect(out).toEqual([{ a: 1 }, { a: 2 }, { a: 3 }]);
	});

	it('decodes multi-byte characters split between chunks', async () => {
		const bytes = new TextEncoder().encode('{"s":"µs"}\n');
		const body = new ReadableStream<Uint8Array>({
			start(c) {
				c.enqueue(bytes.slice(0, 7));
				c.enqueue(bytes.slice(7));
				c.close();
			}
		});
		const out = [];
		for await (const v of ndjson(body)) out.push(v);
		expect(out).toEqual([{ s: 'µs' }]);
	});
});
