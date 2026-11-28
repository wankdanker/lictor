<script lang="ts">
	import { resolve } from '$app/paths';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { toasts } from '#lib/toasts.svelte.js';
	import { hostPort } from '#lib/format.js';
	import CheckActions from '#lib/components/CheckActions.svelte';
	import ConfirmButton from '#lib/components/ConfirmButton.svelte';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';

	const services = new Poll(() => api.localServices());
	const status = new Poll(() => api.status(), 10000);

	async function deregister(id: string) {
		try {
			await api.deregister(id);
			toasts.ok(`Deregistered ${id}`);
			await services.refresh();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<h1>This node</h1>
<div class="toolbar">
	<p class="muted intro">
		Services registered on
		<span class="mono">{status.data?.node?.node_id ?? 'this agent'}</span>, whether or not they are
		currently discoverable. Changes here are what <code>lictor services register</code> and
		<code>lictor checks</code> do.
	</p>
	<span class="spacer"></span>
	<a class="button primary" href={resolve('/agent/services/new')}>Register service</a>
</div>
<ErrorNotice error={services.error} />

{#if services.data?.length}
	<table>
		<thead>
			<tr><th>Service</th><th>Address</th><th>Health</th><th>Checks</th><th></th></tr>
		</thead>
		<tbody>
			{#each services.data as svc (svc.service_id)}
				<tr>
					<td>
						<a class="mono" href={resolve('/agent/services/[id]', { id: svc.service_id })}>
							{svc.service_id}
						</a>
						{#if svc.service_name !== svc.service_id}
							<div class="muted small">name: <span class="mono">{svc.service_name}</span></div>
						{/if}
						<span class="chips">
							{#each svc.tags as tag (tag)}<span class="chip">{tag}</span>{/each}
						</span>
					</td>
					<td class="mono">{hostPort(svc.address, svc.port)}</td>
					<td><StatusBadge status={svc.status} /></td>
					<td>
						{#each svc.checks as check (check.id)}
							<div class="check">
								<div class="check-id">
									<StatusBadge status={check.status} />
									<span class="mono small">{check.id}</span>
									<span class="muted small">{check.type}</span>
								</div>
								{#if check.output}<div class="output mono">{check.output}</div>{/if}
								<CheckActions {check} onchange={() => services.refresh()} />
							</div>
						{:else}
							<span class="muted">none</span>
						{/each}
					</td>
					<td>
						<div class="row-actions">
							<a
								class="button small"
								href={resolve('/agent/services/[id]', { id: svc.service_id })}
							>
								Edit
							</a>
							<ConfirmButton
								small
								label="Deregister"
								confirm="Deregister {svc.service_id}"
								onconfirm={() => deregister(svc.service_id)}
							/>
						</div>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else if services.data}
	<p class="empty">
		Nothing is registered on this node yet.
		<a href={resolve('/agent/services/new')}>Register a service</a>.
	</p>
{:else if services.loading}
	<p class="muted">Loading…</p>
{/if}

<style>
	.intro {
		margin: 0;
		max-width: 46rem;
	}
	.small {
		font-size: 0.8rem;
	}
	.button.small {
		font-size: 0.8rem;
		padding: 0.2rem 0.55rem;
	}
	.check {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		padding: 0.35rem 0;
	}
	.check + .check {
		border-top: 1px dashed var(--border);
	}
	.check-id {
		display: flex;
		gap: 0.4rem;
		align-items: baseline;
	}
	.output {
		font-size: 0.75rem;
		color: var(--muted);
		white-space: pre-wrap;
	}
	.row-actions {
		display: flex;
		gap: 0.35rem;
		justify-content: end;
	}
</style>
