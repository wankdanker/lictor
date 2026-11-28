<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { scopeFromUrl, scopeSearch } from '#lib/url.js';
	import HealthBar from '#lib/components/HealthBar.svelte';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import ScopeFilter from '#lib/components/ScopeFilter.svelte';

	let scope = $derived(scopeFromUrl(page.url));
	const services = new Poll(() => api.services(scope));

	let search = $state('');
	let shown = $derived(
		(services.data ?? []).filter(
			(s) => s.name.includes(search.trim()) || s.tags.some((t) => t.includes(search.trim()))
		)
	);
</script>

<h1>Services</h1>
<div class="toolbar">
	<label>
		Search
		<input type="search" placeholder="name or tag" bind:value={search} />
	</label>
	<ScopeFilter />
	<span class="spacer"></span>
	<a class="button primary" href={resolve('/agent/services/new')}>Register service</a>
</div>
<ErrorNotice error={services.error} />

{#if shown.length}
	<table>
		<thead>
			<tr><th>Name</th><th>Instances</th><th>Health</th><th>Tags</th></tr>
		</thead>
		<tbody>
			{#each shown as svc (svc.name)}
				<tr>
					<td>
						<a
							class="mono"
							href="{resolve('/services/[name]', { name: svc.name })}{scopeSearch(scope)}"
						>
							{svc.name}
						</a>
					</td>
					<td>{svc.instances}</td>
					<td><HealthBar counts={svc} /></td>
					<td>
						<span class="chips">
							{#each svc.tags as tag (tag)}<span class="chip">{tag}</span>{/each}
						</span>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else if services.data}
	<p class="empty">
		{search ? 'No service matches.' : 'No services with live instances.'}
		{#if !search}<a href={resolve('/agent/services/new')}>Register one</a> on this node.{/if}
	</p>
{:else if services.loading}
	<p class="muted">Loading…</p>
{/if}
