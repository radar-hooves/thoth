import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig, type Plugin } from 'vite';

// @ts-expect-error process is a Node global, and the app ships no @types/node
const host = process.env.TAURI_DEV_HOST;

// Tailwind kept out of Svelte's scoped component styles. A `<style>` block is a
// virtual module ending `&lang.css`, which @tailwindcss/vite's filter matches;
// under Vite 8 the dev server can hand it the raw .svelte source before Svelte
// has extracted the CSS, and Tailwind's parser 500s on the first script token.
// No component style uses a Tailwind directive (src/app.css is the one entry),
// so the exclusion changes no output.
function tailwindSkippingSvelteStyles(): Plugin[] {
	const plugins = tailwindcss() as Plugin[];
	for (const plugin of plugins) {
		const transform = plugin.transform as { filter?: { id?: { exclude?: RegExp[] } } } | undefined;
		if (plugin.name?.startsWith('@tailwindcss/vite:generate') && transform?.filter?.id) {
			transform.filter.id.exclude = [...(transform.filter.id.exclude ?? []), /\.svelte\?/];
		}
	}
	return plugins;
}

export default defineConfig({
	plugins: [
		...tailwindSkippingSvelteStyles(),
		sveltekit({
			preprocess: vitePreprocess(),
			// A desktop app has no server: the webview loads a static SPA from the
			// bundle, so every route falls back to index.html.
			adapter: adapter({ fallback: 'index.html' }),
			onwarn: (warning, handler) => {
				// A context menu is mouse-only by nature; its keyboard path is the menu key.
				if (warning.code === 'a11y_click_events_have_key_events') return;
				handler(warning);
			}
		})
	],
	// Rust errors stay on screen, and Tauri expects exactly this port.
	clearScreen: false,
	server: {
		port: 1422,
		strictPort: true,
		host: host || false,
		hmr: host ? { protocol: 'ws', host, port: 1423 } : undefined,
		watch: {
			// The Rust side, and the copies of this source a worktree, direnv or
			// git holds, which would otherwise fire spurious full reloads.
			ignored: ['**/src-tauri/**', '**/.claude/**', '**/.direnv/**', '**/.git/**']
		}
	}
});
