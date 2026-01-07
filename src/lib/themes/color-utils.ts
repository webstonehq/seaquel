import type { ThemeColors } from "$lib/types/theme";

/**
 * Parse an oklch string into its components
 * Handles formats like:
 * - "oklch(0.5 0.2 180)"
 * - "oklch(0.5 0.2 180 / 50%)"
 * - "oklch(1 0 0 / 10%)"
 */
export function parseOklch(
	oklch: string
): { l: number; c: number; h: number; alpha?: number } | null {
	const match = oklch.match(
		/oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+%?))?\s*\)/i
	);
	if (!match) return null;

	const [, l, c, h, alpha] = match;
	const result: { l: number; c: number; h: number; alpha?: number } = {
		l: parseFloat(l),
		c: parseFloat(c),
		h: parseFloat(h),
	};

	if (alpha !== undefined) {
		result.alpha = alpha.endsWith("%") ? parseFloat(alpha) / 100 : parseFloat(alpha);
	}

	return result;
}

/**
 * Format oklch components into a string
 */
export function formatOklch(l: number, c: number, h: number, alpha?: number): string {
	if (alpha !== undefined && alpha < 1) {
		return `oklch(${l} ${c} ${h} / ${Math.round(alpha * 100)}%)`;
	}
	return `oklch(${l} ${c} ${h})`;
}

/**
 * Convert oklch to approximate sRGB hex color
 * This is a simplified conversion - for full accuracy, use a color library
 */
export function oklchToHex(oklch: string): string {
	const parsed = parseOklch(oklch);
	if (!parsed) return "#888888";

	const { l, c, h } = parsed;

	// Convert oklch to oklab
	const hRad = (h * Math.PI) / 180;
	const a = c * Math.cos(hRad);
	const b = c * Math.sin(hRad);

	// Convert oklab to linear sRGB (simplified)
	const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
	const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
	const s_ = l - 0.0894841775 * a - 1.291485548 * b;

	const l3 = l_ * l_ * l_;
	const m3 = m_ * m_ * m_;
	const s3 = s_ * s_ * s_;

	let r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
	let g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
	let bl = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3;

	// Convert linear to sRGB
	const toSrgb = (x: number) => {
		if (x <= 0.0031308) return x * 12.92;
		return 1.055 * Math.pow(x, 1 / 2.4) - 0.055;
	};

	r = Math.max(0, Math.min(1, toSrgb(r)));
	g = Math.max(0, Math.min(1, toSrgb(g)));
	bl = Math.max(0, Math.min(1, toSrgb(bl)));

	const toHex = (x: number) =>
		Math.round(x * 255)
			.toString(16)
			.padStart(2, "0");

	return `#${toHex(r)}${toHex(g)}${toHex(bl)}`;
}

/**
 * Convert hex color to oklch
 */
export function hexToOklch(hex: string): string {
	// Parse hex
	const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
	if (!result) return "oklch(0.5 0 0)";

	let r = parseInt(result[1], 16) / 255;
	let g = parseInt(result[2], 16) / 255;
	let b = parseInt(result[3], 16) / 255;

	// Convert sRGB to linear
	const toLinear = (x: number) => {
		if (x <= 0.04045) return x / 12.92;
		return Math.pow((x + 0.055) / 1.055, 2.4);
	};

	r = toLinear(r);
	g = toLinear(g);
	b = toLinear(b);

	// Convert linear sRGB to oklab
	const l_ = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
	const m_ = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
	const s_ = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);

	const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
	const a = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
	const bVal = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;

	// Convert oklab to oklch
	const C = Math.sqrt(a * a + bVal * bVal);
	let H = (Math.atan2(bVal, a) * 180) / Math.PI;
	if (H < 0) H += 360;

	return formatOklch(
		Math.round(L * 1000) / 1000,
		Math.round(C * 1000) / 1000,
		Math.round(H * 1000) / 1000
	);
}

/**
 * Check if a string is a valid oklch color
 */
export function isValidOklch(value: string): boolean {
	return parseOklch(value) !== null;
}

/**
 * Validate that an object has all required theme color keys
 */
export function validateThemeColors(colors: unknown): colors is ThemeColors {
	if (!colors || typeof colors !== "object") return false;

	const requiredKeys: (keyof ThemeColors)[] = [
		"background",
		"foreground",
		"card",
		"cardForeground",
		"popover",
		"popoverForeground",
		"primary",
		"primaryForeground",
		"secondary",
		"secondaryForeground",
		"muted",
		"mutedForeground",
		"accent",
		"accentForeground",
		"destructive",
		"border",
		"input",
		"ring",
		"chart1",
		"chart2",
		"chart3",
		"chart4",
		"chart5",
		"sidebar",
		"sidebarForeground",
		"sidebarPrimary",
		"sidebarPrimaryForeground",
		"sidebarAccent",
		"sidebarAccentForeground",
		"sidebarBorder",
		"sidebarRing",
	];

	const obj = colors as Record<string, unknown>;

	for (const key of requiredKeys) {
		if (typeof obj[key] !== "string") return false;
		// Optionally validate that each is a valid oklch value
		// if (!isValidOklch(obj[key] as string)) return false;
	}

	return true;
}

/**
 * Get a human-readable label for a color key
 */
export function getColorLabel(key: keyof ThemeColors): string {
	const labels: Record<keyof ThemeColors, string> = {
		background: "Background",
		foreground: "Foreground",
		card: "Card",
		cardForeground: "Card Text",
		popover: "Popover",
		popoverForeground: "Popover Text",
		primary: "Primary",
		primaryForeground: "Primary Text",
		secondary: "Secondary",
		secondaryForeground: "Secondary Text",
		muted: "Muted",
		mutedForeground: "Muted Text",
		accent: "Accent",
		accentForeground: "Accent Text",
		destructive: "Destructive",
		border: "Border",
		input: "Input",
		ring: "Ring",
		chart1: "Chart 1",
		chart2: "Chart 2",
		chart3: "Chart 3",
		chart4: "Chart 4",
		chart5: "Chart 5",
		sidebar: "Sidebar",
		sidebarForeground: "Sidebar Text",
		sidebarPrimary: "Sidebar Primary",
		sidebarPrimaryForeground: "Sidebar Primary Text",
		sidebarAccent: "Sidebar Accent",
		sidebarAccentForeground: "Sidebar Accent Text",
		sidebarBorder: "Sidebar Border",
		sidebarRing: "Sidebar Ring",
	};
	return labels[key];
}
