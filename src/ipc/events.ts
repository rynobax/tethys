import { useTauriEvent } from "../useTauriEvent";
import type {
  ArtifactChangedEvent,
  GithubAuthSnapshot,
  GithubStatusChangedEvent,
  Theme,
  TurnChangedEvent,
} from "../types";

/** Checked against Rust's emit sites by `scripts/check-ipc-parity.mjs`. */
export interface AppEvents {
  "workspace:changed": { workspace_id: string };
  "session:changed": { workspace_id: string };
  "session:exit": {
    workspace_id: string;
    session_id: string;
    code: number | null;
  };
  "session:turn_changed": TurnChangedEvent;
  "github:auth_changed": GithubAuthSnapshot;
  "github:status_changed": GithubStatusChangedEvent;
  "system_status:changed": null;
  "pending_permissions:changed": null;
  "theme:changed": Theme | null;
  "artifact:changed": ArtifactChangedEvent;
}

export type AppEventName = keyof AppEvents;

export function useAppEvent<K extends AppEventName>(
  name: K,
  handler: (payload: AppEvents[K]) => void,
): void {
  useTauriEvent<AppEvents[K]>(name, (event) => handler(event.payload));
}
