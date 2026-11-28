<script lang="ts">
	// The fields of one health check definition.

	import type { CheckForm, Errors } from '#lib/definition.js';
	import { HEALTH_ORDER } from '#lib/types.js';
	import PairsEditor from './PairsEditor.svelte';

	let {
		check = $bindable(),
		errors = {},
		prefix
	}: {
		check: CheckForm;
		errors?: Errors;
		/** Error path prefix, e.g. `checks.0`. */
		prefix: string;
	} = $props();

	const err = (field: string) => errors[`${prefix}.${field}`];
	let advanced = $state(false);
	const uid = $props.id();
</script>

{#snippet field(name: string, label: string, placeholder = '', hint = '')}
	{@const error = err(name)}
	{@const note = `${uid}-${name}-note`}
	<div class="field">
		<label>
			{label}
			<input
				bind:value={check[name as 'id']}
				{placeholder}
				aria-invalid={error ? 'true' : undefined}
				aria-describedby={error || hint ? note : undefined}
			/>
		</label>
		{#if error}<span class="field-error" id={note}>{error}</span>
		{:else if hint}<span class="hint" id={note}>{hint}</span>{/if}
	</div>
{/snippet}

<div class="check">
	<div class="grid">
		<label>
			Type
			<select bind:value={check.type}>
				<option value="http">HTTP</option>
				<option value="tcp">TCP</option>
				<option value="ttl">TTL (reported by the app)</option>
			</select>
		</label>
		{#if check.type === 'http'}
			<div class="wide">{@render field('url', 'URL', 'http://127.0.0.1:8080/health')}</div>
			{@render field('method', 'Method', 'GET')}
		{:else if check.type === 'tcp'}
			<div class="wide">{@render field('address', 'Address', '127.0.0.1:8080')}</div>
		{:else}
			{@render field('ttl', 'TTL', '30s', 'Critical unless reported within this time')}
		{/if}
		{#if check.type !== 'ttl'}
			{@render field('interval', 'Interval', 'agent default', 'e.g. 10s')}
			{@render field('timeout', 'Timeout', 'agent default')}
		{/if}
	</div>

	<button
		type="button"
		class="link"
		aria-expanded={advanced}
		onclick={() => (advanced = !advanced)}
	>
		{advanced ? 'Fewer options' : 'More options'}
	</button>

	{#if advanced}
		<div class="grid">
			{@render field('id', 'Check id', 'service:<id>')}
			{@render field('name', 'Name', 'same as the id')}
			<label>
				Initial status
				<select bind:value={check.initial_status}>
					{#each HEALTH_ORDER as s (s)}<option value={s}>{s}</option>{/each}
				</select>
			</label>
			<label>
				Failures before critical
				<input type="number" min="1" bind:value={check.failures_before_critical} />
				{#if err('failures_before_critical')}
					<span class="field-error">{err('failures_before_critical')}</span>
				{/if}
			</label>
			<label>
				Successes before passing
				<input type="number" min="1" bind:value={check.successes_before_passing} />
				{#if err('successes_before_passing')}
					<span class="field-error">{err('successes_before_passing')}</span>
				{/if}
			</label>
			{@render field(
				'deregister_critical_after',
				'Deregister when critical for',
				'never',
				'Removes the whole service'
			)}
		</div>
		{#if check.type === 'http'}
			<label class="inline">
				<input type="checkbox" bind:checked={check.tls_skip_verify} />
				Accept invalid TLS certificates
			</label>
			<PairsEditor
				bind:pairs={check.headers}
				label="Headers"
				keyLabel="Header"
				{errors}
				prefix="{prefix}.headers"
			/>
		{/if}
		<label>
			Notes
			<input bind:value={check.notes} placeholder="For operators" />
		</label>
	{/if}
</div>

<style>
	.check {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr));
		gap: 0.75rem;
	}
	.wide {
		grid-column: span 2;
	}
	.wide :global(input) {
		width: 100%;
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
	.link {
		align-self: start;
		border: 0;
		background: none;
		padding: 0;
		color: var(--accent);
		font-size: 0.85rem;
	}
	.link:hover:not(:disabled) {
		background: none;
		text-decoration: underline;
	}
</style>
