/**
 * Toast utilities with consistent behavior across the app.
 */

import { toast } from 'svelte-sonner';
import ErrorToast from '$lib/components/error-toast.svelte';

/**
 * Shows an error toast that doesn't auto-close and includes a copy button.
 * Error toasts require manual dismissal to ensure users see important errors.
 */
export function errorToast(message: string): void {
	toast.error(ErrorToast, {

    componentProps: {
			message
    },
    // duration: Infinity
	});
}
