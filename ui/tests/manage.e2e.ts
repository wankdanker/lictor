// The UI against a real `lictor agent --dev` (see playwright.config.ts).

import { expect, test, type Page } from '@playwright/test';

/** Register a service through the API, for tests that are not about the register form. */
async function register(page: Page, body: object) {
	const res = await page.request.post('/v1/agent/services', { data: body });
	expect(res.status()).toBe(201);
}

test('the overview shows this node and the dev backend', async ({ page }) => {
	await page.goto('/');
	await expect(page).toHaveURL(/\/ui\/$/);
	await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
	await expect(page.getByRole('status').filter({ hasText: 'Dev mode (in memory)' })).toBeVisible();
	await expect(page.getByText('memory', { exact: true })).toBeVisible();
});

test('register a service with a TTL check, report it, and see it in the catalog', async ({
	page
}) => {
	await page.goto('/ui/agent/services/new');
	await page.getByLabel('Name', { exact: true }).fill('e2e-web');
	await page.getByLabel('Port', { exact: true }).fill('8080');
	await page.getByLabel('Tags', { exact: true }).fill('v2');
	await page.getByLabel('Tags', { exact: true }).press('Enter');
	await page.getByRole('button', { name: 'Add check' }).click();
	await page.getByLabel('Type').selectOption('ttl');
	await page.getByLabel('TTL', { exact: true }).fill('1m');
	await page.getByRole('button', { name: 'Register service' }).click();

	await expect(page).toHaveURL(/\/ui\/agent\/services\/e2e-web$/);
	await expect(page.getByText('Registered e2e-web')).toBeVisible();
	const row = page.getByRole('row').filter({ hasText: 'service:e2e-web' });
	// A new TTL check starts critical, until the app reports.
	await expect(row.getByText('critical')).toBeVisible();
	await row.getByLabel('Output for service:e2e-web').fill('all good');
	await row.getByRole('button', { name: 'Pass service:e2e-web' }).click();
	await expect(row.getByText('passing')).toBeVisible();
	await expect(row.getByText('all good')).toBeVisible();

	await page.getByRole('link', { name: 'Services', exact: true }).click();
	const svc = page.getByRole('row').filter({ hasText: 'e2e-web' });
	await expect(svc).toContainText('v2');
	await svc.getByRole('link', { name: 'e2e-web' }).click();
	await expect(page).toHaveURL(/\/ui\/services\/e2e-web$/);
	await expect(page.getByText(/Live · index \d+/)).toBeVisible();
	await expect(page.getByRole('cell', { name: /:8080/ })).toBeVisible();
});

test('the instance view updates live and filters by health', async ({ page }) => {
	await register(page, {
		name: 'e2e-live',
		port: 81,
		checks: [{ id: 'live-ttl', type: 'ttl', ttl: '1m' }]
	});
	await page.goto('/ui/services/e2e-live');
	await expect(page.getByRole('cell', { name: /:81/ })).toBeVisible();
	await page.getByLabel('Health').selectOption('passing');
	await expect(page.getByText('No live instances of e2e-live match.')).toBeVisible();

	// A change made elsewhere (here, through the API) arrives over the watch stream.
	await page.request.put('/v1/agent/checks/live-ttl/pass');
	await expect(page.getByRole('cell', { name: /:81/ })).toBeVisible();
});

test('edit a definition and add and remove checks', async ({ page }) => {
	await register(page, {
		name: 'e2e-edit',
		port: 9000,
		metadata: { team: 'core' },
		checks: [{ id: 'edit-ttl', type: 'ttl', ttl: '30s' }]
	});
	await page.goto('/ui/agent/services/e2e-edit');
	await page.getByRole('button', { name: 'Edit definition' }).click();
	await expect(page.getByLabel('Id', { exact: true })).toBeDisabled();
	await expect(page.getByLabel('Metadata value 1')).toHaveValue('core');
	await page.getByLabel('Port', { exact: true }).fill('9090');
	await page.getByRole('button', { name: 'Save changes' }).click();
	await expect(page.getByText('Saved e2e-edit')).toBeVisible();
	await expect(page.getByText(':9090')).toBeVisible();
	// The existing check survived the edit.
	await expect(page.getByRole('row').filter({ hasText: 'edit-ttl' })).toBeVisible();

	await page.getByRole('button', { name: 'Add check' }).click();
	const form = page.locator('form.add');
	await form.getByLabel('Type').selectOption('tcp');
	await form.getByLabel('Address').fill('127.0.0.1:1');
	await form.getByRole('button', { name: 'More options' }).click();
	await form.getByLabel('Check id').fill('edit-tcp');
	await form.getByRole('button', { name: 'Add check' }).click();
	await expect(page.getByText('Added check edit-tcp')).toBeVisible();
	const tcp = page.getByRole('row').filter({ hasText: 'edit-tcp' });
	await expect(tcp).toBeVisible();
	// Only TTL checks can be reported.
	await expect(tcp.getByRole('button', { name: /^Pass/ })).toHaveCount(0);

	await tcp.getByRole('button', { name: 'Remove' }).click();
	await tcp.getByRole('button', { name: 'Remove edit-tcp' }).click();
	await expect(page.getByText('Removed check edit-tcp')).toBeVisible();
	await expect(page.getByRole('row').filter({ hasText: 'edit-tcp' })).toHaveCount(0);

	const def = await (await page.request.get('/v1/agent/services/e2e-edit/definition')).json();
	expect(def).toMatchObject({
		port: 9090,
		metadata: { team: 'core' },
		checks: [{ id: 'edit-ttl' }]
	});
});

test('register from JSON shows the agent error for a bad field', async ({ page }) => {
	await page.goto('/ui/agent/services/new');
	await page.getByRole('button', { name: 'JSON' }).click();
	await page.getByLabel(/Service definition/).fill('{"name": "e2e-json", "prot": 1}');
	await page.getByRole('button', { name: 'Register service' }).click();
	await expect(page.getByRole('alert')).toContainText('unknown field');

	await page.getByLabel(/Service definition/).fill('{"name": "e2e-json", "port": 7000}');
	await page.getByRole('button', { name: 'Register service' }).click();
	await expect(page).toHaveURL(/\/ui\/agent\/services\/e2e-json$/);
});

test('deregister a service', async ({ page }) => {
	await register(page, { name: 'e2e-gone', port: 1 });
	await page.goto('/ui/agent');
	const row = page.getByRole('row').filter({ hasText: 'e2e-gone' });
	await row.getByRole('button', { name: 'Deregister' }).click();
	await row.getByRole('button', { name: 'Deregister e2e-gone' }).click();
	await expect(page.getByText('Deregistered e2e-gone')).toBeVisible();
	await expect(page.getByRole('row').filter({ hasText: 'e2e-gone' })).toHaveCount(0);

	// A bookmarked page for a service that is gone says so.
	await page.goto('/ui/agent/services/e2e-gone');
	await expect(page.getByText('No service e2e-gone is registered on this node.')).toBeVisible();
});

test('nodes lists this node with its lease', async ({ page }) => {
	await page.goto('/ui/nodes');
	const row = page.getByRole('row').filter({ hasText: 'this node' });
	await expect(row).toBeVisible();
	await expect(row).toContainText('default/dc1');
	await expect(row).toContainText(/\d+(\.\d)?s/);
});

test('the scope filter is kept in the URL', async ({ page }) => {
	await page.goto('/ui/services');
	await page.getByLabel('Region').fill('elsewhere');
	await page.getByLabel('Region').press('Enter');
	await expect(page).toHaveURL(/region=elsewhere/);
	await expect(page.getByText('No services with live instances.')).toBeVisible();
	await page.goto('/ui/services?region=dc1');
	await expect(page.getByLabel('Region')).toHaveValue('dc1');
	await expect(page.getByRole('row').filter({ hasText: 'e2e-web' })).toBeVisible();
});

test('unknown pages and files', async ({ page }) => {
	await page.goto('/ui/no/such/page');
	await expect(page.getByRole('heading', { name: 'Page not found' })).toBeVisible();
	const res = await page.request.get('/ui/_app/immutable/missing.js');
	expect(res.status()).toBe(404);
	// The UI must not be framed by other sites.
	const ui = await page.request.get('/ui');
	expect(ui.headers()['x-frame-options']).toBe('DENY');
});
