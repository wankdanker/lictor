<script lang="ts">
	import { formatPairs } from '#lib/format.js';

	let { map, empty = '' }: { map: Record<string, string> | undefined; empty?: string } = $props();

	let pairs = $derived(formatPairs(map));
</script>

{#if pairs.length}
	<span class="chips">
		{#each pairs as pair (pair)}
			<span class="chip">{pair}</span>
		{/each}
	</span>
{:else if empty}
	<span class="muted">{empty}</span>
{/if}
