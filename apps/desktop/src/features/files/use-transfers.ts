import { useCallback, useEffect, useRef, useState } from "react";

import { useDesktopApi, type FileTransfer, type FileTransferEvent } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { applyTransferEvent, isFinished, nextRate, transferTitle, type Rate } from "./list";

export type TransfersState =
  | { status: "loading" }
  | { status: "ready"; transfers: FileTransfer[] }
  | { status: "error"; error: string };

/** What a screen reader hears when a transfer ends. */
function announcement(transfer: FileTransfer): string {
  const title = transferTitle(transfer);
  switch (transfer.status.kind) {
    case "completed":
      return transfer.direction === "incoming" ? `Received ${title}` : `Sent ${title}`;
    case "cancelled":
      return `Cancelled ${title}`;
    default:
      return `Couldn't transfer ${title}`;
  }
}

/**
 * Follows a phone's file transfers: the current list, live changes, and the
 * speed of the running ones. `announcement` is text for an ARIA live region,
 * set when a transfer ends.
 *
 * Render it under `key={deviceId}` so that switching phones starts afresh.
 */
export function useTransfers(deviceId: string) {
  const api = useDesktopApi();
  const [state, setState] = useState<TransfersState>({ status: "loading" });
  const [rates, setRates] = useState<Record<string, Rate>>({});
  const [live, setLive] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const transfers = useRef<FileTransfer[]>([]);
  // Transfers removed from the list, which a late change must not bring
  // back.
  const removed = useRef(new Set<string>());
  const announced = useRef(0);

  useEffect(() => {
    let current = true;
    // Events can arrive before the first list does, so they wait for it.
    let loaded = false;
    const pending: FileTransferEvent[] = [];

    const apply = (event: FileTransferEvent) => {
      if (event.type === "removed") {
        removed.current.add(event.id);
      } else if (event.type === "changed" && removed.current.has(event.transfer.id)) {
        // Only a transfer that starts again under its id comes back.
        if (isFinished(event.transfer)) return;
        removed.current.delete(event.transfer.id);
      } else if (event.type === "progress" && removed.current.has(event.id)) {
        return;
      }
      if (event.type === "changed") {
        const before = transfers.current.find((t) => t.id === event.transfer.id);
        if (isFinished(event.transfer) && !(before && isFinished(before))) {
          // A text that changes, so the same words are read out again.
          announced.current += 1;
          const repeat = announced.current % 2 === 0 ? "\u200b" : "";
          setLive(announcement(event.transfer) + repeat);
        }
      }
      if (event.type === "changed" && event.transfer.status.kind !== "running") {
        // Measured again from the next bytes, not across the pause.
        const { id } = event.transfer;
        setRates((r) => Object.fromEntries(Object.entries(r).filter(([key]) => key !== id)));
      } else if (event.type === "progress" || event.type === "changed") {
        const id = event.type === "progress" ? event.id : event.transfer.id;
        const done = event.type === "progress" ? event.done : event.transfer.done;
        setRates((r) => ({ ...r, [id]: nextRate(r[id], done, Date.now()) }));
      }
      transfers.current = applyTransferEvent(transfers.current, event);
      setState({ status: "ready", transfers: transfers.current });
    };
    const unsubscribe = api.subscribeTransfers(deviceId, (event) => {
      if (!current) return;
      if (loaded) apply(event);
      else pending.push(event);
    });

    api.listTransfers(deviceId).then(
      (list) => {
        if (!current) return;
        loaded = true;
        transfers.current = list;
        setState({ status: "ready", transfers: list });
        pending.splice(0).forEach(apply);
      },
      (cause: unknown) => {
        if (current) setState({ status: "error", error: errorMessage(cause) });
      },
    );

    return () => {
      current = false;
      unsubscribe();
    };
  }, [api, deviceId, attempt]);

  /** Loads the list again, for example after it failed to load. */
  const reload = useCallback(() => {
    setState({ status: "loading" });
    setAttempt((n) => n + 1);
  }, []);

  const attemptTo = useCallback((what: string, action: () => Promise<unknown>) => {
    action().catch((cause: unknown) => {
      setError(`Couldn't ${what}: ${errorMessage(cause)}`);
    });
  }, []);

  /** Opens the file chooser and sends what the user picks. */
  const send = useCallback(() => {
    setChoosing(true);
    attemptTo("send the files", () =>
      api.pickFilesToSend(deviceId).finally(() => {
        setChoosing(false);
      }),
    );
  }, [api, deviceId, attemptTo]);

  const cancel = useCallback(
    (transfer: FileTransfer) => {
      attemptTo("cancel the transfer", () => api.cancelTransfer(deviceId, transfer.id));
    },
    [api, deviceId, attemptTo],
  );

  const forget = useCallback(
    (transfer: FileTransfer) => {
      attemptTo("remove the transfer", () => api.forgetTransfer(deviceId, transfer.id));
    },
    [api, deviceId, attemptTo],
  );

  const open = useCallback(
    (transfer: FileTransfer, fileIndex: number) => {
      attemptTo("open the file", () => api.openReceivedFile(deviceId, transfer.id, fileIndex));
    },
    [api, deviceId, attemptTo],
  );

  const show = useCallback(
    (transfer: FileTransfer, fileIndex: number) => {
      attemptTo("show the file", () => api.showReceivedFile(deviceId, transfer.id, fileIndex));
    },
    [api, deviceId, attemptTo],
  );

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  return {
    state,
    rates,
    announcement: live,
    error,
    clearError,
    reload,
    choosing,
    send,
    cancel,
    forget,
    open,
    show,
  };
}
