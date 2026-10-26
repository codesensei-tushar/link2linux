import { Upload } from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";
import { useNavigate } from "react-router";

import { Button, Callout } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";
import { useDesktopApi, type Device, type FilesDropped, type Unsubscribe } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { PhoneChoiceDialog } from "./PhoneChoiceDialog";
import "./files.css";

/** The phones files can go to: every paired one the phone has not removed. */
function targets(devices: Device[]): Device[] {
  return devices.filter((d) => d.status !== "revoked");
}

/** The phone a drop goes to without asking: the only connected one, or the only one. */
function obviousTarget(phones: Device[]): Device | undefined {
  const connected = phones.filter((d) => d.status === "connected");
  if (connected.length === 1) return connected[0];
  return phones.length === 1 ? phones[0] : undefined;
}

/**
 * Files dropped anywhere on the window go to a phone. While files are dragged
 * over the window it says so; after a drop it sends them to the obvious phone,
 * or asks which one, then opens the files page to show the transfer.
 */
export function FileDropOverlay() {
  const api = useDesktopApi();
  const navigate = useNavigate();
  const { devices } = useDevices();
  const phones = devices.status === "ready" ? targets(devices.data) : [];
  const [over, setOver] = useState(false);
  const [asking, setAsking] = useState<FilesDropped | null>(null);
  const [error, setError] = useState<string | null>(null);

  const sendTo = (dropped: FilesDropped, phone: Device) => {
    setAsking(null);
    api.sendDroppedFiles(dropped.dropId, phone.id).then(
      () => {
        void navigate("/files", { state: { deviceId: phone.id } });
      },
      (cause: unknown) => {
        setError(`Couldn't send the files: ${errorMessage(cause)}`);
      },
    );
  };

  // Reads the phones as they are when the drop happens.
  const onDrop = useEffectEvent((dropped: FilesDropped) => {
    setOver(false);
    setError(null);
    const target = obviousTarget(phones);
    if (target) sendTo(dropped, target);
    else if (phones.length === 0) setError("Pair a phone to send files to it.");
    else setAsking(dropped);
  });

  useEffect(() => {
    let active = true;
    const stops: Unsubscribe[] = [];
    const keep = (stop: Unsubscribe) => {
      if (active) stops.push(stop);
      else stop();
    };
    void api.onFileDrag(setOver).then(keep);
    void api
      .onFilesDropped((dropped) => {
        onDrop(dropped);
      })
      .then(keep);
    return () => {
      active = false;
      stops.forEach((stop) => {
        stop();
      });
    };
  }, [api]);

  const target = obviousTarget(phones);
  return (
    <>
      {over && (
        <div className="file-drop" aria-hidden="true">
          <div className="file-drop__panel">
            <Upload className="file-drop__icon" />
            <p>{target ? `Drop to send to ${target.name}` : "Drop to send to your phone"}</p>
          </div>
        </div>
      )}
      {asking && (
        <PhoneChoiceDialog
          names={asking.names}
          phones={phones}
          onChoose={(phone) => {
            sendTo(asking, phone);
          }}
          onCancel={() => {
            setAsking(null);
          }}
        />
      )}
      {error && (
        <Callout tone="warning" role="alert" className="file-drop__toast">
          <span>{error}</span>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setError(null);
            }}
          >
            Close
          </Button>
        </Callout>
      )}
    </>
  );
}
