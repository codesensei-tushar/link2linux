import { Send } from "lucide-react";
import { useEffect, useRef, useState, type SyntheticEvent } from "react";

import { Button } from "@/components/ui";

import type { NotificationActionInfo } from "./types";

export interface NotificationActionsProps {
  actions: NotificationActionInfo[];
  /** Runs an action, with the reply text for replies. */
  onRun: (action: NotificationActionInfo, replyText?: string) => void;
}

/** A notification's buttons, with an inline reply field for actions that take text. */
export function NotificationActions({ actions, onRun }: NotificationActionsProps) {
  const [replying, setReplying] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const buttonRefs = useRef(new Map<string, HTMLButtonElement>());
  const [draft, setDraft] = useState("");
  const returnFocusTo = useRef<string | null>(null);

  // Focus moves into the field when it opens, and back to its button when it closes.
  useEffect(() => {
    if (replying) {
      inputRef.current?.focus();
    } else if (returnFocusTo.current) {
      buttonRefs.current.get(returnFocusTo.current)?.focus();
      returnFocusTo.current = null;
    }
  }, [replying]);

  const replyAction = actions.find((a) => a.id === replying);

  const closeReply = () => {
    returnFocusTo.current = replying;
    setReplying(null);
    setDraft("");
  };

  const sendReply = (text: string) => {
    if (!replyAction) return;
    onRun(replyAction, text);
    closeReply();
  };

  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const text = draft.trim();
    if (text) sendReply(text);
  };

  return (
    <div className="notification__actions">
      <div role="group" aria-label="Actions" className="notification__buttons">
        {actions.map((action) => (
          <Button
            key={action.id}
            size="sm"
            ref={(node: HTMLButtonElement | null) => {
              if (node) buttonRefs.current.set(action.id, node);
              else buttonRefs.current.delete(action.id);
            }}
            aria-expanded={action.acceptsReply ? replying === action.id : undefined}
            onClick={() => {
              if (!action.acceptsReply) onRun(action);
              else if (replying === action.id) closeReply();
              else setReplying(action.id);
            }}
          >
            {action.label}
          </Button>
        ))}
      </div>
      {replyAction && (
        <form className="notification__reply" onSubmit={submit}>
          {replyAction.replyChoices.length > 0 && (
            <div role="group" aria-label="Quick replies" className="notification__chips">
              {replyAction.replyChoices.map((choice) => (
                <Button
                  key={choice}
                  size="sm"
                  variant="ghost"
                  className="notification__chip"
                  onClick={() => {
                    sendReply(choice);
                  }}
                >
                  {choice}
                </Button>
              ))}
            </div>
          )}
          <div className="notification__reply-row">
            <input
              ref={inputRef}
              className="notification__reply-input"
              aria-label={`${replyAction.label} text`}
              placeholder="Type a reply"
              value={draft}
              onChange={(event) => {
                setDraft(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.stopPropagation();
                  closeReply();
                }
              }}
            />
            <Button type="submit" size="sm" variant="primary" icon={Send} disabled={!draft.trim()}>
              Send
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}
