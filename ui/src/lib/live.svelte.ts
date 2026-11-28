// Reactive data from the agent: polled reads and streamed watches.
//
// Both must be created while a component initializes (they use $effect), and both start over
// when the reactive values their function reads change, or when the API token changes.

import { ApiError } from './api';
import { api, session } from './session.svelte';
import type { DiscoveryFilter, ServiceInstance } from './types';

function asApiError(e: unknown): ApiError {
	return e instanceof ApiError ? e : new ApiError(0, 'error', String(e));
}

/** The result of `fetcher`, fetched now and every `every` ms while the page is visible. */
export class Poll<T> {
	data = $state.raw<T | undefined>(undefined);
	error = $state.raw<ApiError | null>(null);
	loading = $state(true);
	#fetcher: () => Promise<T>;
	#sequence = 0;

	constructor(fetcher: () => Promise<T>, every = 2000) {
		this.#fetcher = fetcher;
		$effect(() => {
			void session.generation;
			// Reading reactive values in the fetcher before its first await makes this effect
			// depend on them, so a new filter fetches at once.
			this.refresh();
			const timer = setInterval(() => {
				if (!document.hidden) this.refresh();
			}, every);
			return () => clearInterval(timer);
		});
	}

	/** Fetch now. Results of earlier, slower fetches are dropped. */
	async refresh(): Promise<void> {
		const n = ++this.#sequence;
		try {
			const data = await this.#fetcher();
			if (n !== this.#sequence) return;
			this.data = data;
			this.error = null;
		} catch (e) {
			if (n !== this.#sequence) return;
			this.error = asApiError(e);
		} finally {
			if (n === this.#sequence) this.loading = false;
		}
	}
}

/** Live instances of a service from `GET /v1/watch/{name}`, reconnecting after errors. */
export class Watch {
	instances = $state.raw<ServiceInstance[] | undefined>(undefined);
	index = $state(0);
	error = $state.raw<ApiError | null>(null);
	live = $state(false);

	constructor(query: () => { name: string; filter: DiscoveryFilter }, retryMs = 2000) {
		$effect(() => {
			void session.generation;
			const { name, filter } = query();
			const abort = new AbortController();
			this.instances = undefined;
			this.live = false;
			void this.#run(name, filter, abort.signal, retryMs);
			return () => abort.abort();
		});
	}

	async #run(name: string, filter: DiscoveryFilter, signal: AbortSignal, retryMs: number) {
		while (!signal.aborted) {
			try {
				for await (const event of api.watch(name, filter, signal)) {
					this.instances = event.instances;
					this.index = event.index;
					this.error = null;
					this.live = true;
				}
			} catch (e) {
				if (signal.aborted) return;
				this.error = asApiError(e);
			}
			this.live = false;
			if (this.error?.status === 401) return; // wait for a token; a new one restarts the watch
			await new Promise((resolve) => setTimeout(resolve, retryMs));
		}
	}
}
