import adapter from '@sveltejs/adapter-static';
import { relative, sep } from 'node:path';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	compilerOptions: {
		runes: ({ filename }) => {
			const relativePath = relative(import.meta.dirname, filename);
			const pathSegments = relativePath.toLowerCase().split(sep);
			const isExternalLibrary = pathSegments.includes('node_modules');
			return isExternalLibrary ? undefined : true;
		}
	},
	kit: {
		// Output dir is env-overridable so the build can be staged and
		// atomically swapped into `build/` only on success — a failed
		// `npm run build` must never delete the live UI the daemon serves.
		adapter: adapter({
			pages: process.env.SVELTE_OUT || 'build',
			assets: process.env.SVELTE_OUT || 'build',
			fallback: 'index.html'
		}),
		prerender: { entries: [] }
	}
};

export default config;
