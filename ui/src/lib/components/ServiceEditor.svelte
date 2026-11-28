<script lang="ts">
	// Create or edit a service definition, as a form or as JSON.

	import {
		emptyCheck,
		fromDefinition,
		toDefinition,
		validate,
		type ServiceForm
	} from '#lib/definition.js';
	import type { ServiceRegistration } from '#lib/types.js';
	import CheckFields from './CheckFields.svelte';
	import PairsEditor from './PairsEditor.svelte';
	import TagsInput from './TagsInput.svelte';

	let {
		initial,
		editing = false,
		onsubmit
	}: {
		initial: ServiceForm;
		/** Editing an existing service: its id is fixed (a new id would register a second service). */
		editing?: boolean;
		onsubmit: (reg: ServiceRegistration) => Promise<void>;
	} = $props();

	// A copy, so cancelling leaves the caller's form untouched.
	// svelte-ignore state_referenced_locally
	let form = $state(structuredClone($state.snapshot(initial)));
	let mode = $state<'form' | 'json'>('form');
	let json = $state('');
	let jsonError = $state('');
	let submitted = $state(false);
	let busy = $state(false);
	let serverError = $state('');

	let errors = $derived(validate(form));
	let shown = $derived(submitted ? errors : {});
	let definition = $derived(toDefinition(form));

	function showJson() {
		json = JSON.stringify(definition, null, 2);
		jsonError = '';
		mode = 'json';
	}

	/** The JSON as a definition, or null (and why, in `jsonError`). */
	function parseJson(): ServiceRegistration | null {
		try {
			const parsed = JSON.parse(json) as ServiceRegistration;
			if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
				throw new Error('expected one service object');
			}
			if (typeof parsed.name !== 'string') throw new Error('"name" is required');
			// Editing keeps the id: another id would register a second service.
			if (editing) parsed.id = form.id;
			jsonError = '';
			return parsed;
		} catch (e) {
			jsonError = e instanceof Error ? e.message : String(e);
			return null;
		}
	}

	function showForm() {
		const parsed = parseJson();
		if (!parsed) return;
		form = fromDefinition(parsed);
		mode = 'form';
	}

	async function save(reg: ServiceRegistration) {
		busy = true;
		try {
			await onsubmit(reg);
		} catch (err) {
			serverError = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		serverError = '';
		if (mode === 'json') {
			// Sent as written, so the agent reports anything the form would drop (unknown fields).
			const parsed = parseJson();
			if (parsed) await save(parsed);
			return;
		}
		submitted = true;
		if (Object.keys(errors).length) return;
		await save(toDefinition(form));
	}

	const errorCount = $derived(Object.keys(shown).length);
</script>

{#snippet input(
	name: 'name' | 'id' | 'address' | 'protocol',
	label: string,
	placeholder = '',
	hint = ''
)}
	{@const error = shown[name]}
	<div class="field">
		<label>
			{label}
			<input
				name="service-{name}"
				bind:value={form[name]}
				{placeholder}
				disabled={editing && name === 'id'}
				aria-invalid={error ? 'true' : undefined}
				aria-describedby={error || hint ? `service-${name}-note` : undefined}
			/>
		</label>
		{#if error}<span class="field-error" id="service-{name}-note">{error}</span>
		{:else if hint}<span class="hint" id="service-{name}-note">{hint}</span>{/if}
	</div>
{/snippet}

<form onsubmit={submit} novalidate>
	<div class="modes" role="group" aria-label="Edit as">
		<button
			type="button"
			aria-pressed={mode === 'form'}
			onclick={() => mode === 'json' && showForm()}
		>
			Form
		</button>
		<button
			type="button"
			aria-pressed={mode === 'json'}
			onclick={() => mode === 'form' && showJson()}
		>
			JSON
		</button>
	</div>

	{#if mode === 'form'}
		<section class="card grid">
			{@render input('name', 'Name', 'web')}
			{@render input('id', 'Id', 'same as the name', editing ? '' : 'Unique on this node')}
			{@render input('address', 'Address', "the node's address")}
			<div class="field">
				<label>
					Port
					<input
						name="service-port"
						type="number"
						min="0"
						max="65535"
						bind:value={form.port}
						placeholder="0"
						aria-invalid={shown.port ? 'true' : undefined}
						aria-describedby={shown.port ? 'service-port-note' : undefined}
					/>
				</label>
				{#if shown.port}<span class="field-error" id="service-port-note">{shown.port}</span>{/if}
			</div>
			{@render input('protocol', 'Protocol', 'tcp')}
			<div class="wide"><TagsInput bind:tags={form.tags} errors={shown} /></div>
			<div class="wide">
				<PairsEditor bind:pairs={form.metadata} label="Metadata" errors={shown} prefix="metadata" />
			</div>
		</section>

		<h2>Health checks</h2>
		{#each form.checks as check, i (check.uid)}
			<section class="card check" aria-label="Check {i + 1}">
				<div class="check-head">
					<strong>Check {i + 1}</strong>
					<button type="button" class="small" onclick={() => form.checks.splice(i, 1)}>
						Remove check
					</button>
				</div>
				<CheckFields bind:check={form.checks[i]} errors={shown} prefix="checks.{i}" />
			</section>
		{:else}
			<p class="muted">No checks: the service counts as passing while this node is alive.</p>
		{/each}
		<button type="button" onclick={() => form.checks.push(emptyCheck())}>Add check</button>
	{:else}
		<label>
			Service definition (the format of <code>lictor services register --file</code>)
			<textarea
				name="service-json"
				rows="22"
				spellcheck="false"
				bind:value={json}
				aria-invalid={jsonError ? 'true' : undefined}></textarea>
		</label>
		{#if jsonError}<p class="field-error">Invalid definition: {jsonError}</p>{/if}
	{/if}

	{#if serverError}<p class="field-error" role="alert">{serverError}</p>{/if}
	{#if errorCount && mode === 'form'}
		<p class="field-error" role="alert">
			Fix {errorCount === 1 ? 'the highlighted field' : `${errorCount} highlighted fields`} first.
		</p>
	{/if}
	<div class="actions">
		<button class="primary" type="submit" disabled={busy}>
			{busy ? 'Saving…' : editing ? 'Save changes' : 'Register service'}
		</button>
	</div>
</form>

<style>
	form {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		max-width: 60rem;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(13rem, 1fr));
		gap: 0.9rem;
	}
	.wide {
		grid-column: 1 / -1;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
	}
	.hint {
		font-size: 0.75rem;
		color: var(--muted);
	}
	h2 {
		margin-bottom: 0;
	}
	.check {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}
	.check-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
	}
	.modes {
		display: flex;
		gap: 0.25rem;
	}
	.modes button[aria-pressed='true'] {
		background: var(--surface-2);
		border-color: var(--accent);
		font-weight: 600;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	form > button {
		align-self: start;
	}
</style>
