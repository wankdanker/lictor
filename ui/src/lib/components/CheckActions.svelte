<script lang="ts">
	// Report a TTL check (pass, warn, fail with optional output) and remove any check.

	import { api } from '#lib/session.svelte.js';
	import { toasts } from '#lib/toasts.svelte.js';
	import type { CheckView } from '#lib/types.js';
	import ConfirmButton from './ConfirmButton.svelte';

	let { check, onchange }: { check: CheckView; onchange: () => void } = $props();

	let output = $state('');
	let busy = $state(false);

	const verbs = [
		{ verb: 'pass', label: 'Pass', done: 'passing' },
		{ verb: 'warn', label: 'Warn', done: 'warning' },
		{ verb: 'fail', label: 'Fail', done: 'critical' }
	] as const;

	async function report(verb: 'pass' | 'warn' | 'fail', done: string) {
		busy = true;
		try {
			await api.report(check.id, verb, output.trim());
			toasts.ok(`Reported ${check.id} ${done}`);
			output = '';
			onchange();
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}

	async function remove() {
		try {
			await api.removeCheck(check.id);
			toasts.ok(`Removed check ${check.id}`);
			onchange();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<div class="actions">
	{#if check.type === 'ttl'}
		<input
			class="output"
			aria-label="Output for {check.id}"
			placeholder="Output (optional)"
			bind:value={output}
		/>
		{#each verbs as v (v.verb)}
			<button
				class="small"
				disabled={busy}
				aria-label="{v.label} {check.id}"
				onclick={() => report(v.verb, v.done)}
			>
				{v.label}
			</button>
		{/each}
	{/if}
	<ConfirmButton small label="Remove" confirm="Remove {check.id}" onconfirm={remove} />
</div>

<style>
	.actions {
		display: flex;
		gap: 0.35rem;
		align-items: center;
		flex-wrap: wrap;
	}
	.output {
		font-size: 0.8rem;
		padding: 0.2rem 0.4rem;
		width: 11rem;
	}
</style>
