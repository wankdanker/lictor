<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { ApiError } from '#lib/api.js';
	import {
		checkToDefinition,
		emptyCheck,
		fromDefinition,
		validateCheck,
		type Errors,
		type ServiceForm
	} from '#lib/definition.js';
	import { hostPort } from '#lib/format.js';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import { toasts } from '#lib/toasts.svelte.js';
	import type { ServiceRegistration } from '#lib/types.js';
	import CheckActions from '#lib/components/CheckActions.svelte';
	import CheckFields from '#lib/components/CheckFields.svelte';
	import ConfirmButton from '#lib/components/ConfirmButton.svelte';
	import ErrorNotice from '#lib/components/ErrorNotice.svelte';
	import Pairs from '#lib/components/Pairs.svelte';
	import ServiceEditor from '#lib/components/ServiceEditor.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';

	let id = $derived(page.params.id ?? '');
	const service = new Poll(() => api.localService(id));

	// The editor works on the stored definition, fetched when it opens, so it never edits a
	// stale copy (checks added below change the definition).
	let editing = $state<ServiceForm | null>(null);
	let loadingDefinition = $state(false);

	async function edit() {
		loadingDefinition = true;
		try {
			editing = fromDefinition(await api.definition(id));
		} catch (e) {
			toasts.error(e);
		} finally {
			loadingDefinition = false;
		}
	}

	async function save(reg: ServiceRegistration) {
		const receipt = await api.register(reg);
		toasts.ok(`Saved ${receipt.service_id}`);
		editing = null;
		await service.refresh();
	}

	async function deregister() {
		try {
			await api.deregister(id);
			toasts.ok(`Deregistered ${id}`);
			await goto(resolve('/agent'));
		} catch (e) {
			toasts.error(e);
		}
	}

	let newCheck = $state(emptyCheck('ttl'));
	let adding = $state(false);
	let checkErrors = $state<Errors>({});

	async function addCheck(e: SubmitEvent) {
		e.preventDefault();
		const errors: Errors = {};
		validateCheck(newCheck, 'new', errors);
		checkErrors = errors;
		if (Object.keys(errors).length) return;
		try {
			const { check_id } = await api.addCheck(id, checkToDefinition(newCheck));
			toasts.ok(`Added check ${check_id}`);
			newCheck = emptyCheck('ttl');
			adding = false;
			await service.refresh();
		} catch (err) {
			toasts.error(err);
		}
	}

	let notFound = $derived(service.error instanceof ApiError && service.error.status === 404);
</script>

<h1>
	<a href={resolve('/agent')}>This node</a> / <span class="mono">{id}</span>
</h1>

{#if notFound}
	<p class="empty">
		No service <span class="mono">{id}</span> is registered on this node.
		<a href={resolve('/agent/services/new')}>Register one</a>.
	</p>
{:else}
	<ErrorNotice error={service.error} />
{/if}

{#if service.data && !notFound}
	{@const svc = service.data}
	<section class="card summary">
		<div class="summary-head">
			<StatusBadge status={svc.status} />
			<span class="mono">{svc.service_name}</span>
			<span class="mono muted">{hostPort(svc.address, svc.port)} · {svc.protocol}</span>
			<span class="spacer"></span>
			<a class="button" href={resolve('/services/[name]', { name: svc.service_name })}>
				All instances of {svc.service_name}
			</a>
			{#if !editing}
				<button onclick={edit} disabled={loadingDefinition}>Edit definition</button>
			{/if}
			<ConfirmButton label="Deregister" confirm="Deregister {id}" onconfirm={deregister} />
		</div>
		<div class="chips">
			{#each svc.tags as tag (tag)}<span class="chip">{tag}</span>{/each}
		</div>
		<Pairs map={svc.metadata} />
	</section>

	{#if editing}
		<h2>Edit definition</h2>
		<p class="muted">
			Saving re-registers the service with this definition, checks included.
			<button class="small" onclick={() => (editing = null)}>Cancel</button>
		</p>
		<ServiceEditor initial={editing} editing onsubmit={save} />
	{:else}
		<h2>Checks</h2>
		{#if svc.checks.length}
			<table>
				<thead>
					<tr><th>Check</th><th>Type</th><th>Health</th><th>Output</th><th></th></tr>
				</thead>
				<tbody>
					{#each svc.checks as check (check.id)}
						<tr>
							<td>
								<span class="mono">{check.id}</span>
								{#if check.name !== check.id}<div class="muted small">{check.name}</div>{/if}
							</td>
							<td>{check.type}</td>
							<td><StatusBadge status={check.status} /></td>
							<td class="mono small output">{check.output ?? ''}</td>
							<td><CheckActions {check} onchange={() => service.refresh()} /></td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p class="muted">No checks: the service counts as passing while this node is alive.</p>
		{/if}

		{#if adding}
			<form class="card add" onsubmit={addCheck} novalidate>
				<h3>Add a check</h3>
				<CheckFields bind:check={newCheck} errors={checkErrors} prefix="new" />
				<div class="actions">
					<button class="primary" type="submit">Add check</button>
					<button type="button" onclick={() => (adding = false)}>Cancel</button>
				</div>
			</form>
		{:else}
			<button class="add-button" onclick={() => (adding = true)}>Add check</button>
		{/if}
	{/if}
{:else if service.loading}
	<p class="muted">Loading…</p>
{/if}

<style>
	.summary {
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}
	.summary-head {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		flex-wrap: wrap;
	}
	.small {
		font-size: 0.8rem;
	}
	.output {
		white-space: pre-wrap;
		color: var(--muted);
	}
	.add {
		margin-top: 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		max-width: 60rem;
	}
	h3 {
		margin: 0;
		font-size: 1rem;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	.add-button {
		margin-top: 1rem;
	}
</style>
