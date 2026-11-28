<script lang="ts">
	import type { Errors, Pair } from '#lib/definition.js';

	let {
		pairs = $bindable(),
		label,
		keyLabel = 'Key',
		valueLabel = 'Value',
		errors = {},
		prefix
	}: {
		pairs: Pair[];
		label: string;
		keyLabel?: string;
		valueLabel?: string;
		errors?: Errors;
		/** Error path prefix, e.g. `metadata` for `metadata.0`. */
		prefix: string;
	} = $props();
</script>

<fieldset>
	<legend>{label}</legend>
	{#each pairs as pair, i (pair)}
		{@const error = errors[`${prefix}.${i}`]}
		<div class="row">
			<input
				aria-label="{label} {keyLabel.toLowerCase()} {i + 1}"
				placeholder={keyLabel}
				bind:value={pair.key}
				aria-invalid={error ? 'true' : undefined}
			/>
			<input
				aria-label="{label} {valueLabel.toLowerCase()} {i + 1}"
				placeholder={valueLabel}
				bind:value={pair.value}
			/>
			<button
				type="button"
				class="small"
				aria-label="Remove {label.toLowerCase()} {i + 1}"
				onclick={() => pairs.splice(i, 1)}
			>
				✕
			</button>
		</div>
		{#if error}<div class="field-error">{error}</div>{/if}
	{/each}
	{#if errors[prefix]}<div class="field-error">{errors[prefix]}</div>{/if}
	<button type="button" class="small" onclick={() => pairs.push({ key: '', value: '' })}>
		Add {label.toLowerCase()}
	</button>
</fieldset>

<style>
	fieldset {
		border: 0;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		align-items: start;
	}
	legend {
		font-size: 0.85rem;
		color: var(--muted);
		padding: 0;
		margin-bottom: 0.25rem;
	}
	.row {
		display: grid;
		grid-template-columns: minmax(6rem, 1fr) minmax(8rem, 2fr) auto;
		gap: 0.4rem;
		width: 100%;
	}
</style>
