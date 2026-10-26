import { ArrowDownToLine, ArrowUpFromLine, ExternalLink, FolderOpen, ListX, X } from "lucide-react";

import { Button } from "@/components/ui";
import { relativeTime } from "@/features/notifications/relative-time";
import type { FileTransfer } from "@/lib/api";
import { cn } from "@/lib/utils";

import { formatBytes, isFinished, shownRate, statusText, transferTitle, type Rate } from "./list";

export interface TransferCardProps {
  transfer: FileTransfer;
  /** The phone on the other end, for the status line. */
  phoneName: string;
  rate: Rate | undefined;
  now: number;
  onCancel: (transfer: FileTransfer) => void;
  onForget: (transfer: FileTransfer) => void;
  onOpen: (transfer: FileTransfer, fileIndex: number) => void;
  onShow: (transfer: FileTransfer, fileIndex: number) => void;
}

/** One transfer: what goes which way, how far it got, and what can be done with it. */
export function TransferCard({
  transfer,
  phoneName,
  rate,
  now,
  onCancel,
  onForget,
  onOpen,
  onShow,
}: TransferCardProps) {
  const title = transferTitle(transfer);
  const outgoing = transfer.direction === "outgoing";
  const Icon = outgoing ? ArrowUpFromLine : ArrowDownToLine;
  const { kind } = transfer.status;
  const finished = isFinished(transfer);

  const amount =
    kind === "completed"
      ? formatBytes(transfer.size)
      : `${formatBytes(transfer.done)} of ${formatBytes(transfer.size)}`;
  const perSecond = kind === "running" ? shownRate(rate, now) : null;
  const speed = perSecond === null ? null : `${formatBytes(Math.round(perSecond))}/s`;
  const details = [statusText(transfer, phoneName), amount, speed].filter(Boolean).join(" · ");
  // Files are offered once the transfer is over: those that arrived, by
  // name when the transfer has several.
  const saved = finished
    ? transfer.files.flatMap((file, index) => (file.saved ? [{ file, index }] : []))
    : [];
  const [firstSaved] = saved;
  const single = transfer.files.length === 1;

  return (
    <article
      className={cn("transfer", `transfer--${kind}`)}
      aria-label={`${outgoing ? "To" : "From"} ${phoneName}: ${title}`}
    >
      <div className={cn("transfer__icon", outgoing && "transfer__icon--outgoing")}>
        <Icon aria-hidden="true" />
      </div>
      <div className="transfer__main">
        <div className="transfer__header">
          <h3 className="transfer__title">{title}</h3>
          <span>{relativeTime(transfer.startedUnixMs, now)}</span>
        </div>
        <p className="transfer__details">{details}</p>
        {!finished && (
          <progress
            className="transfer__bar"
            aria-label={`Progress of ${title}`}
            value={transfer.done}
            max={Math.max(transfer.size, 1)}
          />
        )}
        {saved.length > 0 && !single && (
          <ul className="transfer__files" aria-label={`Files of ${title}`}>
            {saved.map(({ file, index }) => (
              <li key={index}>
                <span className="transfer__file-name">{file.name}</span>
                <span className="transfer__file-size">{formatBytes(file.size)}</span>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={ExternalLink}
                  aria-label={`Open ${file.name}`}
                  onClick={() => {
                    onOpen(transfer, index);
                  }}
                >
                  Open
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="transfer__actions">
          {!finished && (
            <Button
              size="sm"
              icon={X}
              onClick={() => {
                onCancel(transfer);
              }}
            >
              Cancel
            </Button>
          )}
          {firstSaved && single && (
            <Button
              size="sm"
              variant="primary"
              icon={ExternalLink}
              onClick={() => {
                onOpen(transfer, firstSaved.index);
              }}
            >
              Open
            </Button>
          )}
          {firstSaved && (
            <Button
              size="sm"
              icon={FolderOpen}
              onClick={() => {
                onShow(transfer, firstSaved.index);
              }}
            >
              Show in folder
            </Button>
          )}
          {finished && (
            <Button
              size="sm"
              variant="ghost"
              icon={ListX}
              onClick={() => {
                onForget(transfer);
              }}
            >
              Remove from list
            </Button>
          )}
        </div>
      </div>
    </article>
  );
}
