// The API token and the client every page uses.

import { Client } from './api';

const KEY = 'lictor.token';

function storage(kind: 'session' | 'local'): Storage | null {
	try {
		return kind === 'session' ? globalThis.sessionStorage : globalThis.localStorage;
	} catch {
		return null; // storage disabled, or not a browser (unit tests)
	}
}

function saved(): string | null {
	return storage('session')?.getItem(KEY) ?? storage('local')?.getItem(KEY) ?? null;
}

class Session {
	/** Bearer token for the agent API, if it needs one. */
	token = $state<string | null>(saved());
	/** The agent answered 401: the app shows the token dialog. */
	needsToken = $state(false);
	/** Bumped whenever the token changes, so pages fetch again. */
	generation = $state(0);

	/** Use `token` from now on; `remember` keeps it after the browser closes. */
	setToken(token: string, remember: boolean) {
		storage('session')?.removeItem(KEY);
		storage('local')?.removeItem(KEY);
		storage(remember ? 'local' : 'session')?.setItem(KEY, token);
		this.token = token;
		this.needsToken = false;
		this.generation++;
	}

	forget() {
		storage('session')?.removeItem(KEY);
		storage('local')?.removeItem(KEY);
		this.token = null;
		this.generation++;
	}
}

export const session = new Session();

export const api = new Client({
	token: () => session.token,
	onUnauthorized: () => {
		session.needsToken = true;
	}
});
