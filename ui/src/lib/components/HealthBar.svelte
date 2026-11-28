<script lang="ts">
	import { HEALTH_ORDER } from '#lib/types.js';

	interface Counts {
		passing: number;
		warning: number;
		unknown: number;
		critical: number;
	}

	let { counts }: { counts: Counts } = $props();

	let total = $derived(HEALTH_ORDER.reduce((sum, s) => sum + counts[s], 0));
	let label = $derived(
		HEALTH_ORDER.filter((s) => counts[s])
			.map((s) => `${counts[s]} ${s}`)
			.join(', ') || 'no instances'
	);
</script>

<div class="health" role="img" aria-label={label} title={label}>
	<div class="bar">
		{#each HEALTH_ORDER as s (s)}
			{#if counts[s]}
				<span class={s} style:flex-grow={counts[s]}></span>
			{/if}
		{/each}
	</div>
	<span class="counts">
		{#each HEALTH_ORDER as s (s)}
			{#if counts[s]}<span class={['count', s]}>{counts[s]}</span>{/if}
		{/each}
		{#if !total}<span class="muted">0</span>{/if}
	</span>
</div>

<style>
	.health {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		min-width: 8rem;
	}
	.bar {
		display: flex;
		flex: 1;
		height: 0.5rem;
		border-radius: 999px;
		overflow: hidden;
		background: var(--surface-2);
	}
	.bar span {
		flex-basis: 0;
	}
	.counts {
		display: flex;
		gap: 0.4rem;
		font-size: 0.8rem;
		font-variant-numeric: tabular-nums;
	}
	.passing {
		background: var(--passing);
	}
	.warning {
		background: var(--warning);
	}
	.unknown {
		background: var(--unknown);
	}
	.critical {
		background: var(--critical);
	}
	.count {
		background: none;
		font-weight: 600;
	}
	.count.passing {
		color: var(--passing);
	}
	.count.warning {
		color: var(--warning);
	}
	.count.unknown {
		color: var(--unknown);
	}
	.count.critical {
		color: var(--critical);
	}
</style>
