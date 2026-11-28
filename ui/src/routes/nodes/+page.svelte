<script lang="ts">
	import { page } from '$app/state';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { formatMs } from '#lib/format.js';
	import { scopeFromUrl } from '#lib/url.js';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import Pairs from '#lib/components/Pairs.svelte';
	import ScopeFilter from '#lib/components/ScopeFilter.svelte';

	let scope = $derived(scopeFromUrl(page.url));
	const nodes = new Poll(() => api.nodes(scope));
	const status = new Poll(() => api.status(), 10000);
	let here = $derived(status.data?.node?.node_id);

	let search = $state('');
	let shown = $derived(
		(nodes.data ?? []).filter((n) =>
			[n.node_id, n.hostname, ...n.addresses].some((v) => v.includes(search.trim()))
		)
	);
	/** The longest remaining lease, to scale the bars. */
	let longest = $derived(Math.max(1, ...(nodes.data ?? []).map((n) => n.lease_remaining_ms)));
</script>

<h1>Nodes</h1>
<div class="toolbar">
	<label>
		Search
		<input type="search" placeholder="id, hostname or address" bind:value={search} />
	</label>
	<ScopeFilter />
</div>
<ErrorNotice error={nodes.error} />

{#if shown.length}
	<table>
		<thead>
			<tr>
				<th>Node</th>
				<th>Addresses</th>
				<th>Scope</th>
				<th>Services</th>
				<th>Lease</th>
				<th>Agent</th>
				<th>Metadata</th>
			</tr>
		</thead>
		<tbody>
			{#each shown as n (`${n.namespace}.${n.region}.${n.node_id}`)}
				<tr>
					<td>
						<span class="mono">{n.node_id}</span>
						{#if n.node_id === here}<span class="chip">this node</span>{/if}
						<div class="muted small">{n.hostname}</div>
					</td>
					<td class="mono">{n.addresses.join(', ') || '—'}</td>
					<td class="mono">{n.namespace}/{n.region}</td>
					<td>{n.services}</td>
					<td>
						<div
							class="lease"
							title="Expires without a heartbeat in {formatMs(n.lease_remaining_ms)}"
						>
							<span style:width="{(100 * n.lease_remaining_ms) / longest}%"></span>
						</div>
						<span class="muted small">{formatMs(n.lease_remaining_ms)}</span>
					</td>
					<td>
						{n.agent_version}
						<div class="muted small mono">{n.generation}</div>
					</td>
					<td><Pairs map={n.metadata} /></td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else if nodes.data}
	<p class="empty">{search ? 'No node matches.' : 'No live nodes.'}</p>
{:else if nodes.loading}
	<p class="muted">Loading…</p>
{/if}

<style>
	.small {
		font-size: 0.8rem;
	}
	.lease {
		width: 6rem;
		height: 0.4rem;
		border-radius: 999px;
		background: var(--surface-2);
		overflow: hidden;
	}
	.lease span {
		display: block;
		height: 100%;
		background: var(--passing);
	}
</style>
