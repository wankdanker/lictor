import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import process from 'node:process';
import { defineConfig } from '@playwright/test';
import { PORT, TOKEN, TOKEN_PORT } from './tests/env';

// End-to-end tests drive the UI embedded in a real `lictor agent --dev`. Build the UI and then
// the binary first (`npm run build && cargo build -p lictor`), or point LICTOR_BIN at one.
const bin =
	process.env.LICTOR_BIN ??
	resolve(
		import.meta.dirname,
		'../target/debug',
		process.platform === 'win32' ? 'lictor.exe' : 'lictor'
	);

function agent(port: number, ...extra: string[]) {
	const dataDir = mkdtempSync(join(tmpdir(), 'lictor-e2e-'));
	return {
		command: [
			bin,
			'agent',
			'--dev',
			'--http-addr',
			`127.0.0.1:${port}`,
			'--data-dir',
			dataDir,
			...extra
		]
			.map((a) => JSON.stringify(a))
			.join(' '),
		url: `http://127.0.0.1:${port}/ui`,
		reuseExistingServer: false,
		stdout: 'ignore' as const,
		stderr: 'pipe' as const
	};
}

export default defineConfig({
	testDir: 'tests',
	testMatch: '**/*.e2e.ts',
	// The tests share one agent; each uses its own service names, but run them in order.
	workers: 1,
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
	use: {
		baseURL: `http://127.0.0.1:${PORT}`,
		trace: 'retain-on-failure'
	},
	webServer: [agent(PORT), agent(TOKEN_PORT, '--token', TOKEN)]
});
