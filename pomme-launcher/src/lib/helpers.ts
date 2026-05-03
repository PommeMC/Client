export function formatDate(ts: number) {
  return new Date(ts * 1000).toLocaleDateString();
}

export function formatRelativeDate(ts: number) {
  const now = Date.now();
  const diffMs = ts * 1000 - now;
  const isPast = diffMs < 0;
  const absMs = Math.abs(diffMs);

  const minutes = Math.floor(absMs / (1000 * 60));
  const hours = Math.floor(absMs / (1000 * 60 * 60));
  const days = Math.floor(absMs / (1000 * 60 * 60 * 24));
  const weeks = Math.floor(days / 7);

  const date = new Date(ts * 1000);
  const nowDate = new Date(now);
  const months =
    (nowDate.getFullYear() - date.getFullYear()) * 12 + (nowDate.getMonth() - date.getMonth());

  const wrap = (text: string) => (isPast ? `${text} ago` : `in ${text}`);

  if (minutes < 1) return "now";
  if (minutes < 60) return wrap(`${minutes} min`);
  if (hours < 24) return wrap(`${hours} h`);
  if (days === 1) return isPast ? "yesterday" : "tomorrow";
  if (days < 7) return wrap(`${days} days`);
  if (weeks === 1) return wrap("1 week");
  if (weeks < 4) return wrap(`${weeks} weeks`);
  if (months === 1) return wrap("1 month");
  if (months < 4) return wrap(`${months} months`);

  return date.toLocaleDateString();
}

export function normalizeDirectoryName(name: string): string {
  let normalized = name
    .toLowerCase()
    .trim()
    .replace(/\s+/g, "-")

    .replace(/[<>:"\/\\|?*\0]/g, "");

  normalized = normalized.replace(/-+/g, "-");

  normalized = normalized.replace(/^-+|-+$/g, "");
  normalized = normalized.replace(/^\.+|\.+$/g, "");

  return normalized;
}

export function isAbsolutePath(path: string): boolean {
  return /^[A-Za-z]:[\\\/]/.test(path) || path.startsWith("/");
}

export function assertNever(x: never): never | void {
  if (import.meta.env.DEV) {
    throw new Error(`Unhandled case: ${x}`);
  } else {
    console.error(`Unhandled case:`, x);
  }
}
