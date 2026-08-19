import { useTauriEvent } from "../useTauriEvent";
import type {
  GithubAuthSnapshot,
  GithubStatusChangedEvent,
  Theme,
  TurnChangedEvent,
} from "../types";

/**
 * Every event the backend emits, with its payload type.
 *
 * The names used to be string literals scattered across eleven
 * `useTauriEvent` calls in four files, each restating a type parameter that
 * nothing validated. Collecting them makes drift a diff you can read: the
 * table is what `scripts/check-ipc-parity.mjs` compares against the Rust
 * emit sites. That check is what surfaced `workspace:reordered`: emitted,
 * listened to by nobody, and documented in a comment as driving a refresh it
 * did not drive. It has since been removed.
 */
export interface AppEvents {
  "workspace:changed": { workspace_id: string };
  "session:changed": { workspace_id: string };
  "session:exit": {
    workspace_id: string;
    session_id: string;
    code: number | null;
  };
  "session:turn_changed": TurnChangedEvent;
  "script:changed": { workspace_id: string };
  "script:exit": {
    workspace_id: string;
    script_id: string;
    code: number | null;
  };
  "github:auth_changed": GithubAuthSnapshot;
  "github:status_changed": GithubStatusChangedEvent;
  "system_status:changed": null;
  "pending_permissions:changed": null;
  "theme:changed": Theme | null;
}

export type AppEventName = keyof AppEvents;

/**
 * Subscribe to a backend event. Same behaviour as `useTauriEvent`, but the
 * name is checked against `AppEvents` and the payload type comes with it
 * rather than being asserted at the call site.
 */
export function useAppEvent<K extends AppEventName>(
  name: K,
  handler: (payload: AppEvents[K]) => void,
): void {
  useTauriEvent<AppEvents[K]>(name, (event) => handler(event.payload));
}
