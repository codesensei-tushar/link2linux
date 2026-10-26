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
 * or asks which one, then opens the files page to show the transfer. It says
 * why when a drop goes nowhere.
 */
export function FileDropOverlay() {
  const api = useDesktopApi();
  const navigate = useNavigate();
  const { devices } = useDevices();
  const phones = devices.status === "ready" ? targets(devices.data) : [];
  const [over, setOver] = useState(false);
  const [asking, setAsking] = useState<FilesDropped | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const sendTo = (dropped: FilesDropped, phone: Device) => {
    setAsking(null);
    api.sendDroppedFiles(dropped.dropId, phone.id).then(
      () => {
        void navigate("/files", { state: { deviceId: phone.id } });
      },
      (cause: unknown) => {
        setNotice(`Couldn't send the files: ${errorMessage(cause)}`);
      },
    );
  };

  // Reads the phones as they are when the drop happens.
  const onDrop = useEffectEvent((dropped: FilesDropped) => {
    setOver(false);
    // The desktop keeps only the newest drop.
    setNotice(asking ? "Only the newest files you dropped are kept." : null);
    setAsking(null);
    if (devices.status === "loading") {
      setNotice("Still looking for your phones. Drop the files again in a moment.");
      return;
    }
    if (devices.status === "error") {
      setNotice("Your phones couldn't be listed, so the files weren't sent.");
      return;
    }
    const target = obviousTarget(phones);
    if (target) sendTo(dropped, target);
    else if (phones.length === 0) setNotice("Pair a phone to send files to it.");
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

  // The window hears nothing while files are dragged over it, so anything
  // it hears means the drag is over, even when the desktop did not say.
  useEffect(() => {
    if (!over) return;
    const clear = () => {
      setOver(false);
    };
    const kinds = ["pointermove", "keydown", "blur"] as const;
    kinds.forEach((kind) => {
      window.addEventListener(kind, clear);
    });
    return () => {
      kinds.forEach((kind) => {
        window.removeEventListener(kind, clear);
      });
    };
  }, [over]);

  const target = obviousTarget(phones);
  let hint = "Drop to send to one of your phones";
  if (target) hint = `Drop to send to ${target.name}`;
  else if (devices.status === "ready" && phones.length === 0) hint = "Pair a phone to send files";
  // Every phone went away while the question was open.
  const gone = asking !== null && devices.status === "ready" && phones.length === 0;
  const message = gone ? "Your phones are gone, so the files weren't sent." : notice;
  return (
    <>
      {over && (
        <div className="file-drop" aria-hidden="true">
          <div className="file-drop__panel">
            <Upload className="file-drop__icon" />
            <p>{hint}</p>
          </div>
        </div>
      )}
      {asking && phones.length > 0 && (
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
      {message && (
        <Callout tone="warning" role="alert" className="file-drop__toast">
          <span>{message}</span>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setNotice(null);
              if (gone) setAsking(null);
            }}
          >
            Close
          </Button>
        </Callout>
      )}
    </>
  );
}
