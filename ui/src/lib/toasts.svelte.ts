// Short notices after an action ("Registered web", or an error).

export interface Toast {
	id: number;
	kind: 'ok' | 'error';
	text: string;
}

let next = 1;

class Toasts {
	items = $state<Toast[]>([]);

	push(kind: Toast['kind'], text: string, ms = kind === 'error' ? 8000 : 3000) {
		const id = next++;
		this.items.push({ id, kind, text });
		setTimeout(() => this.dismiss(id), ms);
	}

	ok(text: string) {
		this.push('ok', text);
	}

	error(e: unknown) {
		this.push('error', e instanceof Error ? e.message : String(e));
	}

	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
}

export const toasts = new Toasts();
