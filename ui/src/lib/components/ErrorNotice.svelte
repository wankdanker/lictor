<script lang="ts">
	import type { ApiError } from '#lib/api.js';

	let { error }: { error: ApiError | null } = $props();
</script>

{#if error}
	<div class="notice" role="alert">
		<strong>{error.status === 0 ? 'Agent unreachable' : `Error (${error.code})`}</strong>
		{error.message}
	</div>
{/if}

<style>
	.notice {
		padding: 0.6rem 0.9rem;
		margin-bottom: 1rem;
		border-radius: var(--radius);
		border: 1px solid var(--critical);
		background: var(--critical-bg);
		color: var(--text);
	}
	strong {
		color: var(--critical);
		margin-right: 0.35rem;
	}
</style>
