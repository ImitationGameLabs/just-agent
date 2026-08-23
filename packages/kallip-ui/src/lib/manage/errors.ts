// Discriminate typed server errors from raw failures for store error state.
//
// KallipError carries the server envelope message (safe to display as-is);
// anything else (transport, decode, local assembly) is logged with the scope
// tag and replaced by the caller's qualitative catalog fallback.
import { KallipError } from "@kallipai/kallip-common";

export function displayError(
  scope: string,
  e: unknown,
  fallback: string,
): string {
  if (e instanceof KallipError) return e.message;
  console.error(`[${scope}] request failed:`, e);
  return fallback;
}
