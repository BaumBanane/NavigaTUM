export function persistentlyStore(name: string, value: string): void {
  localStorage.setItem(name, value);
  document.cookie = `${name}=${value};Max-Age=31536000;SameSite=Strict;Path=/`;
}