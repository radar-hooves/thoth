import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit()],
	test: {
		globals: true,
		environment: 'jsdom',
		passWithNoTests: true,
		// A saturated CI host once held a synchronous first test past the 5s default.
		testTimeout: 20_000,
		setupFiles: ['src/test/setup.ts'],
		include: ['src/**/*.{test,spec}.{js,ts}']
	},
	resolve: { conditions: ['browser'] }
});
