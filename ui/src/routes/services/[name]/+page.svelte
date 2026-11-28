<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { Poll, Watch } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { hostPort } from '#lib/format.js';
	import { scopeFromUrl } from '#lib/url.js';
	import type { DiscoveryFilter, HealthFilter } from '#lib/types.js';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import Pairs from '#lib/components/Pairs.svelte';
	import ScopeFilter from '#lib/components/ScopeFilter.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';

	let name = $derived(page.params.name ?? '');
	let health = $state<HealthFilter>('any');
	let tagText = $state('');
	let node = $state('');

	let filter = $derived<DiscoveryFilter>({
		...scopeFromUrl(page.url),
		health,
		tags: tagText.split(/[\s,]+/).filter(Boolean),
		node: node.trim() || undefined
	});

	const watch = new Watch(() => ({ name, filter }));
	const status = new Poll(() => api.status(), 10000);
	let here = $derived(status.data?.node?.node_id);
</script>

<h1>
	<a href={resolve('/services')}>Services</a> / <span class="mono">{name}</span>
</h1>

<div class="toolbar">
	<label>
		Health
		<select bind:value={health}>
			<option value="any">any</option>
			<option value="passing">passing</option>
			<option value="not-critical">not critical</option>
		</select>
	</label>
	<label>
		Tags
		<input placeholder="all of: v2, primary" bind:value={tagText} />
	</label>
	<label>
		Node
		<input placeholder="any" bind:value={node} />
	</label>
	<ScopeFilter />
	<span class="spacer"></span>
	<span class={['live', { on: watch.live }]} role="status">
		{watch.live ? `Live · index ${watch.index}` : 'Reconnecting…'}
	</span>
</div>
<ErrorNotice error={watch.error} />

{#if watch.instances?.length}
	<table>
		<thead>
			<tr>
				<th>Instance</th>
				<th>Node</th>
				<th>Address</th>
				<th>Health</th>
				<th>Checks</th>
				<th>Tags &amp; metadata</th>
			</tr>
		</thead>
		<tbody>
			{#each watch.instances as inst (`${inst.namespace}.${inst.region}.${inst.node_id}.${inst.service_id}`)}
				<tr>
					<td>
						<span class="mono">{inst.service_id}</span>
						{#if inst.node_id === here}
							<div>
								<a href={resolve('/agent/services/[id]', { id: inst.service_id })}>Manage</a>
							</div>
						{/if}
						<div class="muted small mono">{inst.namespace}/{inst.region}</div>
					</td>
					<td>
						<span class="mono">{inst.node_id}</span>
						{#if inst.node_id === here}<span class="chip">this node</span>{/if}
						<div class="muted small">{inst.node_hostname}</div>
					</td>
					<td class="mono">
						{hostPort(inst.address, inst.port)}<br /><span class="muted small">
							{inst.protocol}
						</span>
					</td>
					<td><StatusBadge status={inst.status} /></td>
					<td>
						{#each inst.checks as check (check.id)}
							<div class="check">
								<StatusBadge status={check.status} />
								<span class="mono small">{check.id}</span>
								<span class="muted small">{check.type}</span>
								{#if check.output}<div class="output mono">{check.output}</div>{/if}
							</div>
						{:else}
							<span class="muted">none</span>
						{/each}
					</td>
					<td>
						<span class="chips">
							{#each inst.tags as tag (tag)}<span class="chip">{tag}</span>{/each}
						</span>
						<Pairs map={inst.metadata} />
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else if watch.instances}
	<p class="empty">No live instances of <span class="mono">{name}</span> match.</p>
{:else if !watch.error}
	<p class="muted">Loading…</p>
{/if}

<style>
	.live {
		font-size: 0.85rem;
		color: var(--muted);
	}
	.live.on {
		color: var(--passing);
	}
	.small {
		font-size: 0.8rem;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: baseline;
		flex-wrap: wrap;
		margin-bottom: 0.25rem;
	}
	.output {
		width: 100%;
		font-size: 0.75rem;
		color: var(--muted);
		white-space: pre-wrap;
		max-height: 4.5em;
		overflow: auto;
	}
	td .chips {
		margin-bottom: 0.3rem;
	}
</style>
