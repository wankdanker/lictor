// Display helpers.

import type { HealthStatus, ServiceSummary } from './types';

/** `24890` → `24.9s`, `90000` → `1m 30s`. */
export function formatMs(ms: number): string {
	if (ms < 1000) return `${Math.round(ms)}ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
	const total = Math.round(ms / 1000);
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	return [h && `${h}h`, m && `${m}m`, s && `${s}s`].filter(Boolean).join(' ');
}

/** `{a: "1"}` → `a=1`, sorted by key. */
export function formatPairs(map: Record<string, string> | undefined): string[] {
	return Object.entries(map ?? {})
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([k, v]) => `${k}=${v}`);
}

/** The worst state with at least one instance, for a summary's badge. */
export function summaryStatus(s: ServiceSummary): HealthStatus {
	if (s.critical) return 'critical';
	if (s.unknown) return 'unknown';
	if (s.warning) return 'warning';
	return 'passing';
}

/** Sum the health counts of several summaries. */
export function totals(list: ServiceSummary[]) {
	const t = { instances: 0, passing: 0, warning: 0, unknown: 0, critical: 0 };
	for (const s of list) {
		t.instances += s.instances;
		t.passing += s.passing;
		t.warning += s.warning;
		t.unknown += s.unknown;
		t.critical += s.critical;
	}
	return t;
}

/** `host:port`, bracketing IPv6 addresses. */
export function hostPort(address: string, port: number): string {
	const host = address.includes(':') ? `[${address}]` : address;
	return port ? `${host}:${port}` : host;
}
