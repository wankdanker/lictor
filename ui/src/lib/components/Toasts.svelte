<script lang="ts">
	import { toasts } from '#lib/toasts.svelte.js';
</script>

<div class="toasts" aria-live="polite">
	{#each toasts.items as toast (toast.id)}
		<div class={['toast', toast.kind]} role={toast.kind === 'error' ? 'alert' : 'status'}>
			<span>{toast.text}</span>
			<button class="small" aria-label="Dismiss" onclick={() => toasts.dismiss(toast.id)}>✕</button>
		</div>
	{/each}
</div>

<style>
	.toasts {
		position: fixed;
		right: 1rem;
		bottom: 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		z-index: 10;
		max-width: min(28rem, calc(100vw - 2rem));
	}
	.toast {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		justify-content: space-between;
		padding: 0.55rem 0.75rem;
		border-radius: var(--radius);
		border: 1px solid var(--border);
		background: var(--surface);
		box-shadow: 0 4px 14px rgb(0 0 0 / 0.12);
	}
	.ok {
		border-left: 4px solid var(--passing);
	}
	.error {
		border-left: 4px solid var(--critical);
	}
	button {
		border: 0;
		background: none;
	}
</style>
