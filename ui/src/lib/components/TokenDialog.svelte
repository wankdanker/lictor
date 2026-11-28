<script lang="ts">
	// Asks for the API token when the agent answers 401 (agents started with --token).

	import { session } from '#lib/session.svelte.js';

	let token = $state('');
	let remember = $state(false);

	/** Open the dialog while the agent wants a token (reruns when `needsToken` changes). */
	function modal(dialog: HTMLDialogElement) {
		if (session.needsToken && !dialog.open) dialog.showModal();
		if (!session.needsToken && dialog.open) dialog.close();
	}

	function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!token.trim()) return;
		session.setToken(token.trim(), remember);
		token = '';
	}
</script>

<dialog {@attach modal} aria-labelledby="token-title" oncancel={(e) => e.preventDefault()}>
	<form onsubmit={submit}>
		<h2 id="token-title">API token required</h2>
		<p class="muted">
			This agent was started with <code>--token</code>. Enter the token to manage it.
			{#if session.token}The token you entered was not accepted.{/if}
		</p>
		<label>
			Token
			<!-- svelte-ignore a11y_autofocus -->
			<input type="password" autocomplete="current-password" bind:value={token} autofocus />
		</label>
		<label class="inline">
			<input type="checkbox" bind:checked={remember} />
			Remember on this device
		</label>
		<div class="actions">
			<button class="primary" type="submit" disabled={!token.trim()}>Continue</button>
		</div>
	</form>
</dialog>

<style>
	dialog {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		color: var(--text);
		padding: 1.25rem;
		width: min(26rem, calc(100vw - 2rem));
	}
	dialog::backdrop {
		background: rgb(0 0 0 / 0.45);
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 0.8rem;
	}
	h2 {
		margin: 0;
	}
	p {
		margin: 0;
	}
	.actions {
		display: flex;
		justify-content: end;
	}
</style>
