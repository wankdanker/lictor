// An agent started with --token: the UI loads, then asks for the token.

import { expect, test } from '@playwright/test';
import { TOKEN, TOKEN_PORT } from './env';

const base = `http://127.0.0.1:${TOKEN_PORT}`;

test('asks for the token, rejects a wrong one, and remembers it for the session', async ({
	page
}) => {
	await page.goto(`${base}/ui`);
	const dialog = page.getByRole('dialog', { name: 'API token required' });
	await expect(dialog).toBeVisible();

	await dialog.getByLabel('Token').fill('wrong');
	await dialog.getByRole('button', { name: 'Continue' }).click();
	await expect(dialog.getByText('The token you entered was not accepted.')).toBeVisible();

	await dialog.getByLabel('Token').fill(TOKEN);
	await dialog.getByRole('button', { name: 'Continue' }).click();
	await expect(dialog).toBeHidden();
	await expect(page.getByRole('status').filter({ hasText: 'Dev mode (in memory)' })).toBeVisible();

	// Kept for this tab's session...
	await page.goto(`${base}/ui/agent`);
	await expect(page.getByRole('heading', { name: 'This node' })).toBeVisible();
	await expect(dialog).toBeHidden();
	// ...and the API still refuses requests without it.
	const res = await page.request.get(`${base}/v1/status`);
	expect(res.status()).toBe(401);
});
