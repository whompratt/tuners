// In-app modal confirm/alert service (real modals with explicit
// verb buttons — confirm()/alert() are banned, and OS dialogs are reserved
// for file pickers). DialogHost (mounted once in the page) renders the
// pending request; confirmDialog/alertDialog resolve when the user acts.

export type DialogRequest = {
  title: string;
  body: string;
  /** Explicit verb for the confirming action ("Delete", "Share", …). */
  verb?: string;
  /** Cancel label; defaults to "Cancel". */
  cancel?: string;
  danger?: boolean;
  /** Alert mode: single OK button, always resolves true. */
  alert?: boolean;
  resolve: (ok: boolean) => void;
};

export const dialogState = $state({ current: null as DialogRequest | null });

function request(req: Omit<DialogRequest, "resolve">): Promise<boolean> {
  return new Promise((resolve) => {
    dialogState.current = { ...req, resolve };
  });
}

export function confirmDialog(opts: {
  title: string;
  body: string;
  verb: string;
  cancel?: string;
  danger?: boolean;
}): Promise<boolean> {
  return request(opts);
}

export function alertDialog(title: string, body: string): Promise<boolean> {
  return request({ title, body, alert: true });
}

export function settle(ok: boolean) {
  dialogState.current?.resolve(ok);
  dialogState.current = null;
}
