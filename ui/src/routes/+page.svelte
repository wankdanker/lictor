<script lang="ts">
	import { resolve } from '$app/paths';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { formatMs, totals } from '#lib/format.js';
	import HealthBar from '#lib/components/HealthBar.svelte';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import Pairs from '#lib/components/Pairs.svelte';

	const status = new Poll(() => api.status());
	const services = new Poll(() => api.services());

	let node = $derived(status.data?.node);
	let sum = $derived(totals(services.data ?? []));
	let failing = $derived((services.data ?? []).filter((s) => s.critical || s.warning));
</script>

<h1>Overview</h1>
<ErrorNotice error={status.error} />

{#if status.data}
	{@const s = status.data}
	<div class="cards">
		<section class="card">
			<h2>This node</h2>
			{#if node}
				<dl>
					<dt>Node</dt>
					<dd class="mono">{node.node_id}</dd>
					<dt>Hostname</dt>
					<dd>{node.hostname}</dd>
					<dt>Addresses</dt>
					<dd class="mono">{node.addresses.join(', ') || '—'}</dd>
					<dt>Scope</dt>
					<dd class="mono">{node.namespace} / {node.region}</dd>
					<dt>Lease</dt>
					<dd>{formatMs(node.lease_remaining_ms)} left</dd>
					<dt>Generation</dt>
					<dd class="mono">{node.generation}</dd>
					<dt>Metadata</dt>
					<dd><Pairs map={node.metadata} empty="—" /></dd>
				</dl>
			{:else}
				<p class="muted">No node: this is a read-only client.</p>
			{/if}
		</section>

		<section class="card">
			<h2>Agent</h2>
			<dl>
				<dt>Version</dt>
				<dd>{s.version}</dd>
				<dt>Backend</dt>
				<dd>{s.backend}{s.connected ? '' : ' (unreachable)'}</dd>
				<dt>Catalog</dt>
				<dd>{s.catalog_ready ? 'loaded' : 'loading…'}, index {s.catalog_index}</dd>
				<dt>Registered here</dt>
				<dd>
					<a href={resolve('/agent')}>{s.local_services} services, {s.local_checks} checks</a>
				</dd>
			</dl>
		</section>

		<section class="card">
			<h2>Catalog</h2>
			<div class="big">
				<a href={resolve('/nodes')}><strong>{s.catalog_nodes}</strong> nodes</a>
				<a href={resolve('/services')}><strong>{services.data?.length ?? 0}</strong> services</a>
				<span><strong>{sum.instances}</strong> instances</span>
			</div>
			<HealthBar counts={sum} />
		</section>
	</div>

	<h2>Needs attention</h2>
	{#if failing.length}
		<table>
			<thead>
				<tr><th>Service</th><th>Instances</th><th>Health</th></tr>
			</thead>
			<tbody>
				{#each failing as svc (svc.name)}
					<tr>
						<td><a href={resolve('/services/[name]', { name: svc.name })}>{svc.name}</a></td>
						<td>{svc.instances}</td>
						<td><HealthBar counts={svc} /></td>
					</tr>
				{/each}
			</tbody>
		</table>
	{:else if services.data}
		<p class="empty">Every instance is passing or unknown.</p>
	{/if}
{:else if status.loading}
	<p class="muted">Loading…</p>
{/if}

<style>
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr));
		gap: 1rem;
	}
	.card h2 {
		margin-top: 0;
	}
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.35rem 1rem;
		margin: 0;
	}
	dt {
		color: var(--muted);
		font-size: 0.85rem;
	}
	dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.big {
		display: flex;
		gap: 1.25rem;
		flex-wrap: wrap;
		margin-bottom: 1rem;
	}
	.big strong {
		font-size: 1.6rem;
		display: block;
		color: var(--text);
	}
	.big a,
	.big span {
		color: var(--muted);
	}
</style>
