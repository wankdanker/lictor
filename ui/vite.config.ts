import process from 'node:process';
import { defineConfig } from 'vitest/config';
import { playwright } from '@vitest/browser-playwright';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

// `npm run dev` proxies API calls to a running agent (`lictor agent --dev`).
const agent = process.env.LICTOR_AGENT ?? 'http://127.0.0.1:7171';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// A single-page app: every route is rendered in the browser from 200.html.
			// crates/lictor/build.rs embeds build/app into the binary; src/ui.rs serves it.
			adapter: adapter({ pages: 'build/app', assets: 'build/app', fallback: '200.html' }),
			// Must match BASE in crates/lictor/src/ui.rs.
			paths: { base: '/ui' }
		})
	],
	server: {
		proxy: { '/v1': { target: agent, changeOrigin: true } }
	},
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'client',
					browser: {
						enabled: true,
						provider: playwright(),
						instances: [{ browser: 'chromium', headless: true }]
					},
					include: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			},
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
