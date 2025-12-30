import type { Handle } from '@sveltejs/kit';
import { paraglideMiddleware } from '$lib/paraglide/server';

const RTL_LOCALES = ['ar', 'he', 'fa', 'ur'];

const handleParaglide: Handle = ({ event, resolve }) => paraglideMiddleware(event.request, ({ request, locale }) => {
	event.request = request;
	const dir = RTL_LOCALES.includes(locale) ? 'rtl' : 'ltr';

	return resolve(event, {
		transformPageChunk: ({ html }) => html
			.replace('%paraglide.lang%', locale)
			.replace('%paraglide.dir%', dir)
	});
});

export const handle: Handle = handleParaglide;
