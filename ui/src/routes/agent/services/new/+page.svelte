<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { emptyService } from '#lib/definition.js';
	import { api } from '#lib/session.svelte.js';
	import { toasts } from '#lib/toasts.svelte.js';
	import type { ServiceRegistration } from '#lib/types.js';
	import ServiceEditor from '#lib/components/ServiceEditor.svelte';

	async function register(reg: ServiceRegistration) {
		const receipt = await api.register(reg);
		toasts.ok(`Registered ${receipt.service_id}`);
		await goto(resolve('/agent/services/[id]', { id: receipt.service_id }));
	}
</script>

<h1><a href={resolve('/agent')}>This node</a> / Register a service</h1>
<p class="muted">Registering an id that already exists on this node replaces that service.</p>
<ServiceEditor initial={emptyService()} onsubmit={register} />
