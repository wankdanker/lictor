// The editable form behind the service editor, and its conversion to and from the API's
// service definition. Validation mirrors crates/lictor-types (validate.rs and health.rs) so
// most mistakes show up before submitting; the agent still has the final word.

import type { CheckDefinition, CheckType, HealthStatus, ServiceRegistration } from './types';

export interface Pair {
	key: string;
	value: string;
}

export interface CheckForm {
	/** Stable key for the editor's list; not sent. */
	uid: number;
	id: string;
	name: string;
	type: CheckType;
	address: string;
	url: string;
	method: string;
	headers: Pair[];
	tls_skip_verify: boolean;
	ttl: string;
	interval: string;
	timeout: string;
	failures_before_critical: number;
	successes_before_passing: number;
	initial_status: HealthStatus;
	deregister_critical_after: string;
	notes: string;
}

export interface ServiceForm {
	id: string;
	name: string;
	address: string;
	/** null or undefined: not set (an empty number input binds to undefined). */
	port: number | null | undefined;
	protocol: string;
	tags: string[];
	metadata: Pair[];
	checks: CheckForm[];
}

let uid = 1;

export function emptyCheck(type: CheckType = 'http'): CheckForm {
	return {
		uid: uid++,
		id: '',
		name: '',
		type,
		address: '',
		url: '',
		method: 'GET',
		headers: [],
		tls_skip_verify: false,
		ttl: '30s',
		interval: '',
		timeout: '',
		failures_before_critical: 1,
		successes_before_passing: 1,
		initial_status: 'critical',
		deregister_critical_after: '',
		notes: ''
	};
}

export function emptyService(): ServiceForm {
	return {
		id: '',
		name: '',
		address: '',
		port: null,
		protocol: '',
		tags: [],
		metadata: [],
		checks: []
	};
}

function toPairs(map: Record<string, string> | undefined): Pair[] {
	return Object.entries(map ?? {}).map(([key, value]) => ({ key, value }));
}

function fromPairs(pairs: Pair[]): Record<string, string> | undefined {
	const kept = pairs.filter((p) => p.key.trim() !== '');
	if (!kept.length) return undefined;
	return Object.fromEntries(kept.map((p) => [p.key.trim(), p.value]));
}

/** Drop empty strings so the agent applies its defaults. */
function opt(value: string): string | undefined {
	const v = value.trim();
	return v === '' ? undefined : v;
}

export function checkFromDefinition(c: CheckDefinition): CheckForm {
	const form = emptyCheck(c.type);
	form.id = c.id ?? '';
	form.name = c.name ?? '';
	form.interval = c.interval ?? '';
	form.timeout = c.timeout ?? '';
	form.failures_before_critical = c.failures_before_critical ?? 1;
	form.successes_before_passing = c.successes_before_passing ?? 1;
	form.initial_status = c.initial_status ?? 'critical';
	form.deregister_critical_after = c.deregister_critical_after ?? '';
	form.notes = c.notes ?? '';
	if (c.type === 'tcp') form.address = c.address;
	if (c.type === 'http') {
		form.url = c.url;
		form.method = c.method ?? 'GET';
		form.headers = toPairs(c.headers);
		form.tls_skip_verify = c.tls_skip_verify ?? false;
	}
	if (c.type === 'ttl') form.ttl = c.ttl;
	return form;
}

export function checkToDefinition(f: CheckForm): CheckDefinition {
	const common = {
		id: opt(f.id),
		name: opt(f.name),
		failures_before_critical:
			f.failures_before_critical === 1 ? undefined : f.failures_before_critical,
		successes_before_passing:
			f.successes_before_passing === 1 ? undefined : f.successes_before_passing,
		initial_status: f.initial_status,
		deregister_critical_after: opt(f.deregister_critical_after),
		notes: opt(f.notes)
	};
	// Interval and timeout apply to probes only; the agent ignores them for TTL checks.
	const probe = { interval: opt(f.interval), timeout: opt(f.timeout) };
	switch (f.type) {
		case 'tcp':
			return clean({ ...common, ...probe, type: 'tcp', address: f.address.trim() });
		case 'http':
			return clean({
				...common,
				...probe,
				type: 'http',
				url: f.url.trim(),
				method: f.method.trim().toUpperCase() || 'GET',
				headers: fromPairs(f.headers),
				tls_skip_verify: f.tls_skip_verify || undefined
			});
		case 'ttl':
			return clean({ ...common, type: 'ttl', ttl: f.ttl.trim() });
	}
}

export function fromDefinition(reg: ServiceRegistration): ServiceForm {
	return {
		id: reg.id ?? '',
		name: reg.name,
		address: reg.address ?? '',
		port: reg.port ?? null,
		protocol: reg.protocol ?? '',
		tags: [...new Set(reg.tags ?? [])],
		metadata: toPairs(reg.metadata),
		checks: (reg.checks ?? []).map(checkFromDefinition)
	};
}

export function toDefinition(f: ServiceForm): ServiceRegistration {
	const tags = f.tags.map((t) => t.trim()).filter(Boolean);
	return clean({
		id: opt(f.id),
		name: f.name.trim(),
		address: opt(f.address),
		port: f.port ?? undefined,
		protocol: opt(f.protocol),
		tags: tags.length ? tags : undefined,
		metadata: fromPairs(f.metadata),
		checks: f.checks.length ? f.checks.map(checkToDefinition) : undefined
	});
}

/** Remove `undefined` fields, so the JSON shows only what was set. */
function clean<T extends object>(o: T): T {
	return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as T;
}

// -- validation ---------------------------------------------------------------------------

const MAX_IDENT = 128;
const MAX_TAG = 255;
const MAX_META_VALUE = 512;
const MAX_META_PAIRS = 64;

/** An error message, or null: 1-128 letters, digits, `_` or `-`, not starting with `-`. */
export function identError(what: string, value: string): string | null {
	if (value === '') return `${what} must not be empty`;
	if (value.length > MAX_IDENT) return `${what} is longer than ${MAX_IDENT} characters`;
	if (value.startsWith('-')) return `${what} must not start with '-'`;
	const bad = [...value].find((c) => !/[A-Za-z0-9_-]/.test(c));
	if (bad !== undefined) return `${what} contains '${bad}'; use letters, digits, '_' or '-'`;
	return null;
}

const UNITS: Record<string, number> = {
	ns: 1e-6,
	us: 1e-3,
	µs: 1e-3,
	ms: 1,
	s: 1000,
	sec: 1000,
	secs: 1000,
	m: 60_000,
	min: 60_000,
	mins: 60_000,
	h: 3_600_000,
	hr: 3_600_000,
	hrs: 3_600_000,
	d: 86_400_000,
	w: 604_800_000
};

/** Milliseconds in a duration such as `10s`, `1m30s` or `500ms` (a bare number is seconds), or null. */
export function parseDuration(text: string): number | null {
	const t = text.trim();
	if (/^\d+$/.test(t)) return Number(t) * 1000;
	const parts = [...t.matchAll(/(\d+(?:\.\d+)?)\s*([a-zµ]+)\s*/gy)];
	if (!parts.length || parts.map((p) => p[0]).join('') !== t) return null;
	let ms = 0;
	for (const [, n, unit] of parts) {
		const factor = UNITS[unit];
		if (factor === undefined) return null;
		ms += Number(n) * factor;
	}
	return ms;
}

/** Field path (`name`, `tags.1`, `checks.0.url`, ...) → message. Empty when valid. */
export type Errors = Record<string, string>;

export function validateCheck(c: CheckForm, prefix: string, errors: Errors) {
	const set = (field: string, message: string | null) => {
		if (message) errors[`${prefix}.${field}`] = message;
	};
	if (c.id.trim()) set('id', identError('check id', c.id.trim().replaceAll(':', '_')));
	for (const field of ['failures_before_critical', 'successes_before_passing'] as const) {
		if (!Number.isInteger(c[field]) || c[field] < 1) set(field, 'must be at least 1');
	}
	const duration = (field: keyof CheckForm & string, min: number, minText: string) => {
		const text = String(c[field]).trim();
		if (!text) return;
		const ms = parseDuration(text);
		if (ms === null) set(field, `'${text}' is not a duration (e.g. 10s, 500ms, 1m30s)`);
		else if (ms < min) set(field, `must be at least ${minText}`);
	};
	duration('deregister_critical_after', 0, '0s');
	if (c.type === 'ttl') {
		if (!c.ttl.trim()) set('ttl', 'a TTL is required');
		duration('ttl', 1000, '1s');
		return;
	}
	duration('interval', 100, '100ms');
	duration('timeout', 100, '100ms');
	if (c.type === 'tcp') {
		const port = c.address.trim().split(':').pop();
		if (!c.address.includes(':') || !/^\d+$/.test(port ?? '') || Number(port) > 65535) {
			set('address', 'must be host:port');
		}
	} else {
		if (!/^https?:\/\//.test(c.url.trim())) set('url', 'must start with http:// or https://');
		if (c.method.trim() && !/^[A-Za-z]+$/.test(c.method.trim())) {
			set('method', 'must be an HTTP method such as GET');
		}
		c.headers.forEach((h, i) => {
			if (!h.key.trim() && h.value.trim()) set(`headers.${i}`, 'header name is empty');
		});
	}
}

export function validate(f: ServiceForm): Errors {
	const errors: Errors = {};
	const set = (field: string, message: string | null) => {
		if (message) errors[field] = message;
	};
	set('name', identError('service name', f.name.trim()));
	if (f.id.trim()) set('id', identError('service id', f.id.trim()));
	if (f.protocol.trim()) set('protocol', identError('protocol', f.protocol.trim()));
	if (/\s/.test(f.address.trim())) set('address', 'address must not contain whitespace');
	if (f.port != null && (!Number.isInteger(f.port) || f.port < 0 || f.port > 65535)) {
		set('port', 'port must be between 0 and 65535');
	}
	f.tags.forEach((t, i) => {
		const tag = t.trim();
		if (!tag) return;
		if (tag.length > MAX_TAG) set(`tags.${i}`, `tag is longer than ${MAX_TAG} characters`);
		else if (/\p{Cc}/u.test(tag)) set(`tags.${i}`, 'tags must not contain control characters');
	});
	const keys = f.metadata.filter((p) => p.key.trim() !== '');
	if (keys.length > MAX_META_PAIRS) set('metadata', `at most ${MAX_META_PAIRS} metadata pairs`);
	const seenKeys = new Set<string>();
	f.metadata.forEach((p, i) => {
		const key = p.key.trim();
		if (!key) {
			if (p.value.trim()) set(`metadata.${i}`, 'metadata key is empty');
			return;
		}
		set(`metadata.${i}`, identError('metadata key', key));
		if (p.value.length > MAX_META_VALUE) {
			set(`metadata.${i}`, `value is longer than ${MAX_META_VALUE} characters`);
		}
		if (seenKeys.has(key)) set(`metadata.${i}`, `duplicate key '${key}'`);
		seenKeys.add(key);
	});
	const seenChecks = new Set<string>();
	f.checks.forEach((c, i) => {
		validateCheck(c, `checks.${i}`, errors);
		const id = c.id.trim();
		if (id && seenChecks.has(id)) errors[`checks.${i}.id`] = `duplicate check id '${id}'`;
		if (id) seenChecks.add(id);
	});
	return errors;
}
