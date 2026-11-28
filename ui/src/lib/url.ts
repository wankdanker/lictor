import type { Scope } from './types';

/** The `?namespace=&region=` filter of a page URL. */
export function scopeFromUrl(url: { searchParams: { get(key: string): string | null } }): Scope {
	return {
		namespace: url.searchParams.get('namespace') || undefined,
		region: url.searchParams.get('region') || undefined
	};
}

/** `?namespace=&region=` for links that keep the current scope ('' when unscoped). */
export function scopeSearch(scope: Scope): string {
	const q = new URLSearchParams();
	if (scope.namespace) q.set('namespace', scope.namespace);
	if (scope.region) q.set('region', scope.region);
	const s = q.toString();
	return s ? `?${s}` : '';
}
