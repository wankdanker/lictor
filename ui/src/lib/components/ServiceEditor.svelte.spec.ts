import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { emptyService, fromDefinition } from '#lib/definition.js';
import type { ServiceRegistration } from '#lib/types.js';
import ServiceEditor from './ServiceEditor.svelte';

function setup(initial = emptyService(), editing = false) {
	const onsubmit = vi.fn<(reg: ServiceRegistration) => Promise<void>>(async () => {});
	render(ServiceEditor, { initial, editing, onsubmit });
	return onsubmit;
}

describe('ServiceEditor', () => {
	it('builds a definition from the form', async () => {
		const onsubmit = setup();
		await page.getByLabelText('Name').fill('web');
		await page.getByLabelText('Port').fill('8080');
		await page.getByLabelText('Tags').fill('v2, primary');
		await userEvent.keyboard('{Enter}');
		await page.getByRole('button', { name: 'Add metadata' }).click();
		await page.getByLabelText('Metadata key 1').fill('shard');
		await page.getByLabelText('Metadata value 1').fill('4');
		await page.getByRole('button', { name: 'Add check' }).click();
		await page.getByLabelText('URL').fill('http://127.0.0.1:8080/health');
		await page.getByLabelText('Interval').fill('5s');
		await page.getByRole('button', { name: 'Register service' }).click();

		await expect.poll(() => onsubmit.mock.calls.length).toBe(1);
		expect(onsubmit.mock.calls[0][0]).toEqual({
			name: 'web',
			port: 8080,
			tags: ['v2', 'primary'],
			metadata: { shard: '4' },
			checks: [
				{
					type: 'http',
					url: 'http://127.0.0.1:8080/health',
					method: 'GET',
					interval: '5s',
					initial_status: 'critical'
				}
			]
		});
	});

	it('shows errors instead of submitting', async () => {
		const onsubmit = setup();
		await page.getByLabelText('Name').fill('bad name');
		await page.getByRole('button', { name: 'Add check' }).click();
		await page.getByRole('button', { name: 'Register service' }).click();

		await expect
			.element(page.getByText("contains ' '; use letters", { exact: false }))
			.toBeVisible();
		await expect.element(page.getByText('must start with http:// or https://')).toBeVisible();
		await expect.element(page.getByText('Fix 2 highlighted fields first.')).toBeVisible();
		expect(onsubmit).not.toHaveBeenCalled();
	});

	it('edits as JSON and sends it as written', async () => {
		const onsubmit = setup();
		await page.getByLabelText('Name').fill('web');
		await page.getByRole('button', { name: 'JSON' }).click();
		const text = page.getByLabelText('Service definition', { exact: false });
		await expect.element(text).toHaveValue('{\n  "name": "web"\n}');
		await text.fill('{"name": "api", "port": 9000, "prot": 1}');
		await page.getByRole('button', { name: 'Register service' }).click();
		await expect.poll(() => onsubmit.mock.calls.length).toBe(1);
		// Unknown fields go to the agent, which rejects them with a clear message.
		expect(onsubmit.mock.calls[0][0]).toEqual({ name: 'api', port: 9000, prot: 1 });

		await page.getByRole('button', { name: 'Form' }).click();
		await expect.element(page.getByLabelText('Name')).toHaveValue('api');
	});

	it('refuses JSON that is not a service', async () => {
		const onsubmit = setup();
		await page.getByRole('button', { name: 'JSON' }).click();
		await page.getByLabelText('Service definition', { exact: false }).fill('[1, 2]');
		await page.getByRole('button', { name: 'Register service' }).click();
		await expect
			.element(page.getByText('Invalid definition: expected one service object'))
			.toBeVisible();
		expect(onsubmit).not.toHaveBeenCalled();
	});

	it('keeps the id fixed when editing', async () => {
		const onsubmit = setup(
			fromDefinition({
				id: 'web',
				name: 'web',
				port: 80,
				checks: [{ id: 'beat', type: 'ttl', ttl: '30s' }]
			}),
			true
		);
		await expect.element(page.getByLabelText('Id')).toBeDisabled();
		await page.getByRole('button', { name: 'JSON' }).click();
		await page
			.getByLabelText('Service definition', { exact: false })
			.fill('{"id": "other", "name": "web", "port": 81}');
		await page.getByRole('button', { name: 'Save changes' }).click();
		await expect.poll(() => onsubmit.mock.calls.length).toBe(1);
		expect(onsubmit.mock.calls[0][0]).toEqual({ id: 'web', name: 'web', port: 81 });
	});

	it('shows what the agent said when saving fails', async () => {
		const onsubmit = vi.fn(async () => {
			throw new Error("service 'web': unknown field `prot`");
		});
		render(ServiceEditor, { initial: fromDefinition({ name: 'web' }), onsubmit });
		await page.getByRole('button', { name: 'Register service' }).click();
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent("service 'web': unknown field `prot`");
	});
});
