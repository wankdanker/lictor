// The agent's API types, mirroring crates/lictor-types (see docs/http-api.md).

export type HealthStatus = 'passing' | 'warning' | 'unknown' | 'critical';

/** Best first, as in the agent: an instance's health is the worst of its checks. */
export const HEALTH_ORDER: HealthStatus[] = ['passing', 'warning', 'unknown', 'critical'];

export type HealthFilter = 'any' | 'passing' | 'not-critical';

export interface CheckView {
	id: string;
	service_id: string;
	name: string;
	type: CheckType;
	status: HealthStatus;
	output?: string;
}

export interface ServiceInstance {
	namespace: string;
	region: string;
	service_id: string;
	service_name: string;
	address: string;
	port: number;
	protocol: string;
	tags: string[];
	metadata: Record<string, string>;
	status: HealthStatus;
	checks: CheckView[];
	node_id: string;
	node_hostname: string;
	node_addresses: string[];
	node_metadata: Record<string, string>;
	generation: string;
	revision: number;
}

export interface ServiceSummary {
	name: string;
	instances: number;
	passing: number;
	warning: number;
	critical: number;
	unknown: number;
	tags: string[];
}

export interface NodeInfo {
	namespace: string;
	region: string;
	node_id: string;
	generation: string;
	hostname: string;
	addresses: string[];
	metadata: Record<string, string>;
	agent_version: string;
	services: number;
	lease_remaining_ms: number;
}

export interface AgentStatus {
	version: string;
	backend: string;
	connected: boolean;
	catalog_ready: boolean;
	node?: NodeInfo;
	local_services: number;
	local_checks: number;
	catalog_nodes: number;
	catalog_services: number;
	catalog_index: number;
}

export type CheckType = 'tcp' | 'http' | 'ttl';

interface CheckCommon {
	id?: string;
	name?: string;
	/** Durations are strings such as `10s`, `500ms` or `1m30s`. */
	interval?: string;
	timeout?: string;
	failures_before_critical?: number;
	successes_before_passing?: number;
	initial_status?: HealthStatus;
	deregister_critical_after?: string;
	notes?: string;
}

export type CheckDefinition = CheckCommon &
	(
		| { type: 'tcp'; address: string }
		| {
				type: 'http';
				url: string;
				method?: string;
				headers?: Record<string, string>;
				tls_skip_verify?: boolean;
		  }
		| { type: 'ttl'; ttl: string }
	);

/** A service definition, as accepted by `POST /v1/agent/services` and `register --file`. */
export interface ServiceRegistration {
	id?: string;
	name: string;
	address?: string;
	port?: number;
	protocol?: string;
	tags?: string[];
	metadata?: Record<string, string>;
	checks?: CheckDefinition[];
}

export interface RegistrationReceipt {
	service_id: string;
	service_name: string;
	key: string;
	revision: number;
	generation: string;
	checks: string[];
}

export interface RegistryEvent {
	index: number;
	instances: ServiceInstance[];
}

export interface Scope {
	namespace?: string;
	region?: string;
}

export interface DiscoveryFilter extends Scope {
	health?: HealthFilter;
	tags?: string[];
	node?: string;
	/** `key:value` pairs. */
	meta?: string[];
}
