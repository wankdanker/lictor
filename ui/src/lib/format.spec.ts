import { describe, expect, it } from 'vitest';
import { formatMs, formatPairs, hostPort, summaryStatus, totals } from './format';
import { scopeFromUrl, scopeSearch } from './url';

const summary = (
	counts: Partial<Record<'passing' | 'warning' | 'unknown' | 'critical', number>>
) => ({
	name: 'x',
	instances: 0,
	tags: [],
	passing: 0,
	warning: 0,
	unknown: 0,
	critical: 0,
	...counts
});

describe('format', () => {
	it('formats lease times', () => {
		expect(formatMs(250)).toBe('250ms');
		expect(formatMs(4321)).toBe('4.3s');
		expect(formatMs(24890)).toBe('25s');
		expect(formatMs(90_000)).toBe('1m 30s');
		expect(formatMs(3_600_000)).toBe('1h');
	});

	it('sorts metadata pairs', () => {
		expect(formatPairs({ b: '2', a: '1' })).toEqual(['a=1', 'b=2']);
		expect(formatPairs(undefined)).toEqual([]);
	});

	it('brackets IPv6 addresses', () => {
		expect(hostPort('10.0.0.1', 80)).toBe('10.0.0.1:80');
		expect(hostPort('fe80::1', 80)).toBe('[fe80::1]:80');
		expect(hostPort('db', 0)).toBe('db');
	});

	it('picks the worst state of a summary', () => {
		expect(summaryStatus(summary({ passing: 2 }))).toBe('passing');
		expect(summaryStatus(summary({ passing: 2, warning: 1 }))).toBe('warning');
		expect(summaryStatus(summary({ unknown: 1, warning: 1 }))).toBe('unknown');
		expect(summaryStatus(summary({ critical: 1, passing: 9 }))).toBe('critical');
	});

	it('adds up summaries', () => {
		expect(totals([summary({ passing: 1 }), summary({ passing: 2, critical: 1 })])).toMatchObject({
			passing: 3,
			critical: 1
		});
	});
});

describe('url scope', () => {
	it('reads and writes the scope query', () => {
		const url = new URL('http://x/ui/services?namespace=prod&region=');
		expect(scopeFromUrl(url)).toEqual({ namespace: 'prod', region: undefined });
		expect(scopeSearch({ namespace: 'prod', region: 'us' })).toBe('?namespace=prod&region=us');
		expect(scopeSearch({})).toBe('');
	});
});
