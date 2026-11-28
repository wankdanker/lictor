<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
</script>

<h1>{page.status === 404 ? 'Page not found' : 'Something went wrong'}</h1>
<p class="muted">{page.error?.message}</p>
<p><a href={resolve('/')}>Back to the overview</a></p>
