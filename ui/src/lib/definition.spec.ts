import { describe, expect, it } from 'vitest';
import {
	checkFromDefinition,
	checkToDefinition,
	emptyCheck,
	emptyService,
	fromDefinition,
	identError,
	parseDuration,
	toDefinition,
	validate
} from './definition';
import type { ServiceRegistration } from './types';

const full: ServiceRegistration = {
	id: 'web-1',
	name: 'web',
	address: '10.0.0.5',
	port: 8080,
	protocol: 'http',
	tags: ['v2', 'primary'],
	metadata: { shard: '4', team: 'core' },
	checks: [
		{
			id: 'web-http',
			name: 'Web health',
			type: 'http',
			url: 'https://127.0.0.1:8080/health',
			method: 'HEAD',
			headers: { 'X-Probe': 'lictor' },
			tls_skip_verify: true,
			interval: '5s',
			timeout: '2s',
			failures_before_critical: 3,
			successes_before_passing: 2,
			initial_status: 'warning',
			deregister_critical_after: '10m',
			notes: 'pager'
		},
		{ id: 'web-tcp', type: 'tcp', address: '127.0.0.1:8080', initial_status: 'critical' },
		{ id: 'beat', type: 'ttl', ttl: '30s', initial_status: 'critical' }
	]
};

describe('definition conversion', () => {
	it('round-trips every field', () => {
		expect(toDefinition(fromDefinition(full))).toEqual(full);
	});

	it('leaves defaults to the agent', () => {
		const form = emptyService();
		form.name = ' web ';
		form.tags = ['', ' v2 '];
		form.metadata = [{ key: '', value: '' }];
		expect(toDefinition(form)).toEqual({ name: 'web', tags: ['v2'] });
	});

	it('drops probe options from TTL checks', () => {
		const c = emptyCheck('ttl');
		c.interval = '5s';
		c.ttl = ' 1m ';
		expect(checkToDefinition(c)).toEqual({ type: 'ttl', ttl: '1m', initial_status: 'critical' });
	});

	it('upper-cases HTTP methods and defaults to GET', () => {
		const c = emptyCheck('http');
		c.url = 'http://x/h';
		c.method = 'post';
		expect(checkToDefinition(c)).toMatchObject({ method: 'POST' });
		c.method = '';
		expect(checkToDefinition(c)).toMatchObject({ method: 'GET' });
	});

	it('fills in defaults from a sparse definition', () => {
		const c = checkFromDefinition({ type: 'ttl', ttl: '30s' });
		expect(c).toMatchObject({ failures_before_critical: 1, initial_status: 'critical', id: '' });
	});

	it('drops duplicate tags (the editor keys tags by value)', () => {
		expect(fromDefinition({ name: 'a', tags: ['x', 'x', 'y'] }).tags).toEqual(['x', 'y']);
	});
});

describe('validation', () => {
	it('accepts a good definition', () => {
		expect(validate(fromDefinition(full))).toEqual({});
	});

	it('mirrors the agent identifier rules', () => {
		expect(identError('service name', 'web_1-a')).toBeNull();
		expect(identError('service name', '')).toBe('service name must not be empty');
		expect(identError('service name', '-web')).toMatch("must not start with '-'");
		expect(identError('service name', 'a.b')).toMatch("contains '.'");
		expect(identError('service name', 'x'.repeat(129))).toMatch('longer than 128');
	});

	it('reports fields by path', () => {
		const form = emptyService();
		form.name = 'bad name';
		form.port = 70000;
		form.tags = ['ok', 'has\ttab', 'dma-New York'];
		form.metadata = [
			{ key: 'a', value: '1' },
			{ key: 'a', value: '2' },
			{ key: '', value: 'orphan' }
		];
		const tcp = emptyCheck('tcp');
		tcp.address = 'localhost';
		tcp.interval = '50ms';
		const http = emptyCheck('http');
		http.url = 'ftp://x';
		http.failures_before_critical = 0;
		const ttl = emptyCheck('ttl');
		ttl.ttl = '500ms';
		ttl.id = 'a.b';
		form.checks = [tcp, http, ttl];
		expect(Object.keys(validate(form)).sort()).toEqual([
			'checks.0.address',
			'checks.0.interval',
			'checks.1.failures_before_critical',
			'checks.1.url',
			'checks.2.id',
			'checks.2.ttl',
			'metadata.1',
			'metadata.2',
			'name',
			'port',
			'tags.1'
		]);
	});

	it('allows colons in check ids and finds duplicates', () => {
		const form = emptyService();
		form.name = 'web';
		const a = emptyCheck('ttl');
		a.id = 'service:web';
		const b = emptyCheck('ttl');
		b.id = 'service:web';
		form.checks = [a, b];
		expect(validate(form)).toEqual({ 'checks.1.id': "duplicate check id 'service:web'" });
	});

	it('treats an empty port as unset', () => {
		const form = emptyService();
		form.name = 'web';
		form.port = undefined;
		expect(validate(form)).toEqual({});
		expect(toDefinition(form)).toEqual({ name: 'web' });
	});
});

describe('parseDuration', () => {
	it.each([
		['30', 30_000],
		['10s', 10_000],
		['500ms', 500],
		['1m30s', 90_000],
		['1m 30s', 90_000],
		['2h', 7_200_000],
		['1.5s', 1500]
	])('%s', (text, ms) => {
		expect(parseDuration(text)).toBe(ms);
	});

	it.each(['', 's', '10 parsecs', '10s junk', '-5s'])('rejects %j', (text) => {
		expect(parseDuration(text)).toBeNull();
	});
});
