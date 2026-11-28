<script lang="ts">
	import '../app.css';
	import favicon from '#lib/assets/favicon.svg';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { Poll } from '#lib/live.svelte.js';
	import { api } from '#lib/session.svelte.js';
	import TokenDialog from '#lib/components/TokenDialog.svelte';
	import Toasts from '#lib/components/Toasts.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const status = new Poll(() => api.status(), 5000);

	const nav = [
		{ href: resolve('/'), label: 'Overview', exact: true },
		{ href: resolve('/services'), label: 'Services' },
		{ href: resolve('/nodes'), label: 'Nodes' },
		{ href: resolve('/agent'), label: 'This node' }
	];

	function active(href: string, exact = false) {
		const path = page.url.pathname.replace(/\/$/, '');
		return exact ? path === href.replace(/\/$/, '') : path.startsWith(href);
	}

	let conn = $derived.by(() => {
		if (status.error) return { kind: 'down', text: 'Agent unreachable' };
		const s = status.data;
		if (!s) return { kind: 'pending', text: 'Connecting…' };
		if (!s.connected) return { kind: 'warn', text: `${s.backend} unreachable` };
		if (!s.catalog_ready) return { kind: 'warn', text: 'Loading catalog' };
		return { kind: 'ok', text: s.backend === 'memory' ? 'Dev mode (in memory)' : 'Connected' };
	});
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>{status.data?.node ? `${status.data.node.node_id} · Lictor` : 'Lictor'}</title>
</svelte:head>

<header>
	<a class="brand" href={resolve('/')}>
		<img src={favicon} alt="" width="22" height="22" />
		Lictor
	</a>
	<nav aria-label="Main">
		{#each nav as item (item.href)}
			<a href={item.href} aria-current={active(item.href, item.exact) ? 'page' : undefined}>
				{item.label}
			</a>
		{/each}
	</nav>
	<span class="spacer"></span>
	{#if status.data?.node}
		<span class="node mono" title="This agent's node">{status.data.node.node_id}</span>
	{/if}
	<span class={['state', conn.kind]} role="status">
		<span class="dot" aria-hidden="true"></span>{conn.text}
	</span>
</header>

<main>
	{@render children()}
</main>

<TokenDialog />
<Toasts />

<style>
	header {
		display: flex;
		align-items: center;
		gap: 1.5rem;
		padding: 0 1.25rem;
		height: 3.25rem;
		background: var(--surface);
		border-bottom: 1px solid var(--border);
		position: sticky;
		top: 0;
		z-index: 5;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-weight: 700;
		color: var(--text);
		font-size: 1.05rem;
	}
	.brand:hover {
		text-decoration: none;
	}
	nav {
		display: flex;
		gap: 0.25rem;
	}
	nav a {
		color: var(--muted);
		padding: 0.35rem 0.7rem;
		border-radius: var(--radius);
	}
	nav a:hover {
		background: var(--surface-2);
		text-decoration: none;
	}
	nav a[aria-current='page'] {
		color: var(--text);
		background: var(--surface-2);
		font-weight: 600;
	}
	.node {
		color: var(--muted);
	}
	.state {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.dot {
		width: 0.55rem;
		height: 0.55rem;
		border-radius: 50%;
		background: var(--unknown);
	}
	.ok .dot {
		background: var(--passing);
	}
	.warn .dot {
		background: var(--warning);
	}
	.down .dot {
		background: var(--critical);
	}
	main {
		padding: 1.5rem 1.25rem 4rem;
		max-width: 80rem;
		margin: 0 auto;
	}
	@media (max-width: 720px) {
		header {
			flex-wrap: wrap;
			height: auto;
			padding: 0.5rem 1rem;
			gap: 0.5rem 1rem;
		}
		.node {
			display: none;
		}
	}
</style>
