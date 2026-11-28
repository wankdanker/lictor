<script lang="ts">
	// A destructive action that takes two clicks: the first asks, the second acts.

	let {
		label,
		confirm = `Confirm ${label.toLowerCase()}`,
		onconfirm,
		small = false
	}: {
		label: string;
		confirm?: string;
		onconfirm: () => Promise<void> | void;
		small?: boolean;
	} = $props();

	let asking = $state(false);
	let busy = $state(false);

	async function act() {
		busy = true;
		try {
			await onconfirm();
		} finally {
			busy = false;
			asking = false;
		}
	}
</script>

{#if asking}
	<span class="confirm">
		<button class={['danger', { small }]} disabled={busy} onclick={act}>{confirm}</button>
		<button class={{ small }} disabled={busy} onclick={() => (asking = false)}>Cancel</button>
	</span>
{:else}
	<button class={['danger', { small }]} onclick={() => (asking = true)}>{label}</button>
{/if}

<style>
	.confirm {
		display: inline-flex;
		gap: 0.35rem;
	}
</style>
