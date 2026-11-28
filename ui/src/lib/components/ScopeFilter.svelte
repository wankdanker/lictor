<script lang="ts">
	// Namespace and region filters, kept in the URL (`?namespace=&region=`) so views can be shared.

	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	function set(key: 'namespace' | 'region', value: string) {
		const url = new URL(page.url.href);
		if (value.trim()) url.searchParams.set(key, value.trim());
		else url.searchParams.delete(key);
		void goto(url, { replace: true, reset: false });
	}
</script>

<label>
	Namespace
	<input
		name="namespace"
		placeholder="all"
		value={page.url.searchParams.get('namespace') ?? ''}
		onchange={(e) => set('namespace', e.currentTarget.value)}
	/>
</label>
<label>
	Region
	<input
		name="region"
		placeholder="all"
		value={page.url.searchParams.get('region') ?? ''}
		onchange={(e) => set('region', e.currentTarget.value)}
	/>
</label>
