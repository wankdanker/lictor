import process from 'node:process';

/** The agent the tests manage. */
export const PORT = Number(process.env.LICTOR_E2E_PORT ?? 17171);
/** A second agent, started with `--token TOKEN`. */
export const TOKEN_PORT = PORT + 1;
export const TOKEN = 'e2e-secret';
