<script lang="ts">
	import type { HealthStatus } from '#lib/types.js';

	let { status }: { status: HealthStatus } = $props();
</script>

<span class={['badge', status]}>{status}</span>

<style>
	.badge {
		display: inline-block;
		font-size: 0.75rem;
		font-weight: 600;
		padding: 0.05rem 0.5rem;
		border-radius: 999px;
		text-transform: lowercase;
	}
	.passing {
		color: var(--passing);
		background: var(--passing-bg);
	}
	.warning {
		color: var(--warning);
		background: var(--warning-bg);
	}
	.unknown {
		color: var(--unknown);
		background: var(--unknown-bg);
	}
	.critical {
		color: var(--critical);
		background: var(--critical-bg);
	}
</style>
