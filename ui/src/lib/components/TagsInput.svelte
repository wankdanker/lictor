<script lang="ts">
	import type { Errors } from '#lib/definition.js';

	let {
		tags = $bindable(),
		errors = {}
	}: {
		tags: string[];
		errors?: Errors;
	} = $props();

	let draft = $state('');

	/** Add what was typed; commas and spaces separate several tags. */
	function commit() {
		const added = draft.split(/[\s,]+/).filter((t) => t && !tags.includes(t));
		tags.push(...added);
		draft = '';
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ',') {
			e.preventDefault();
			commit();
		} else if (e.key === 'Backspace' && draft === '' && tags.length) {
			tags.pop();
		}
	}

	let tagErrors = $derived(
		tags.map((_, i) => errors[`tags.${i}`]).filter((e): e is string => Boolean(e))
	);
</script>

<div class="field">
	<label for="tags-input">Tags</label>
	<div class="box">
		{#each tags as tag, i (tag)}
			<span class={['chip', { bad: errors[`tags.${i}`] }]}>
				{tag}
				<button type="button" aria-label="Remove tag {tag}" onclick={() => tags.splice(i, 1)}>
					✕
				</button>
			</span>
		{/each}
		<input
			id="tags-input"
			placeholder={tags.length ? '' : 'v2, primary…'}
			bind:value={draft}
			{onkeydown}
			onblur={commit}
		/>
	</div>
	{#each tagErrors as e (e)}<div class="field-error">{e}</div>{/each}
</div>

<style>
	.field {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	label {
		display: block;
	}
	.box {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
		align-items: center;
		padding: 0.25rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	.box input {
		border: 0;
		flex: 1;
		min-width: 6rem;
		padding: 0.2rem 0.3rem;
	}
	.box input:focus {
		outline: none;
	}
	.box:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: -1px;
	}
	.chip button {
		border: 0;
		background: none;
		padding: 0 0 0 0.2rem;
		font-size: 0.7rem;
		color: var(--muted);
	}
	.chip.bad {
		border-color: var(--critical);
	}
</style>
