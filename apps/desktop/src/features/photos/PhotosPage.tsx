import { Images, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { Button, EmptyState, PageHeader, Spinner, StatusDot } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";
import type { Device, Photo, SavedPhoto } from "@/lib/api";

import { photoKey, photosNotice } from "./format";
import { photosMeta } from "./meta";
import { PhotoGrid } from "./PhotoGrid";
import { PhotoViewer } from "./PhotoViewer";
import { usePhotos } from "./use-photos";
import "./photos.css";

/** One phone's photos, or why they can't be shown. */
function PhotoLibrary({ phone }: { phone: Device }) {
  const [state, reload] = usePhotos(phone.id);
  const [openId, setOpenId] = useState<string | null>(null);
  // A save that just finished, before the list event that carries it arrives.
  const [justSaved, setJustSaved] = useState<Record<string, SavedPhoto>>({});

  const listed = state.status === "ready" && state.data.state === "listed" ? state.data.photos : [];
  const open = listed.find((photo) => photo.id === openId);
  // A photo that left the list closes the viewer, for good.
  if (openId !== null && !open) setOpenId(null);

  if (state.status === "loading") return <Spinner label="Loading photos…" />;
  if (state.status === "error") {
    return (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load photos"
        description={state.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  }
  const status = state.data.state;
  if (status === "waiting") {
    return <Spinner label={`Getting the list of photos from ${phone.name}…`} />;
  }
  if (status !== "listed") return <EmptyState {...photosNotice(status, phone.name)} />;
  if (listed.length === 0) {
    return (
      <EmptyState
        icon={Images}
        title="No recent photos"
        description={`Photos and screenshots you take with ${phone.name} show up here.`}
      />
    );
  }

  const select = (photo: Photo) => {
    setOpenId(photo.id);
  };
  return (
    <>
      <p className="photos-summary">
        {listed.length === 1
          ? `The latest photo from ${phone.name}`
          : `The ${listed.length} latest photos and screenshots from ${phone.name}`}
      </p>
      <PhotoGrid phone={phone} photos={listed} onOpen={select} />
      {open && (
        <PhotoViewer
          phone={phone}
          photos={listed}
          photo={open}
          saved={justSaved[photoKey(open)] ?? open.saved ?? undefined}
          onSaved={(photo, result) => {
            setJustSaved((before) => ({ ...before, [photoKey(photo)]: result }));
          }}
          onSelect={select}
          onClose={() => {
            setOpenId(null);
          }}
        />
      )}
    </>
  );
}

/** A choice between paired phones, shown when there are several. */
function PhonePicker({
  phones,
  selected,
  onSelect,
}: {
  phones: Device[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  return (
    <fieldset className="photos-picker">
      <legend className="sr-only">Phone</legend>
      {phones.map((phone) => (
        <label key={phone.id} className="photos-picker__option">
          <input
            type="radio"
            name="photos-phone"
            value={phone.id}
            checked={phone.id === selected}
            onChange={() => {
              onSelect(phone.id);
            }}
          />
          <StatusDot tone={phone.status === "connected" ? "success" : "neutral"} />
          {phone.name}
          <span className="sr-only">
            {phone.status === "connected" ? ", connected" : ", not connected"}
          </span>
        </label>
      ))}
    </fieldset>
  );
}

export function PhotosPage() {
  const { devices } = useDevices();
  const [chosen, setChosen] = useState<string | null>(null);
  const phones =
    devices.status === "ready" ? devices.data.filter((d) => d.status !== "revoked") : [];
  const phone =
    phones.find((p) => p.id === chosen) ??
    phones.find((p) => p.status === "connected") ??
    phones[0];

  return (
    <>
      <PageHeader title={photosMeta.label} description={photosMeta.description} />
      <div className="page-body">
        {devices.status === "loading" ? (
          <Spinner label="Looking for your phone…" />
        ) : phone ? (
          <>
            {phones.length > 1 && (
              <PhonePicker phones={phones} selected={phone.id} onSelect={setChosen} />
            )}
            <PhotoLibrary key={phone.id} phone={phone} />
          </>
        ) : (
          <EmptyState
            icon={Smartphone}
            title="No phone paired"
            description="Pair your phone to see its recent photos here and save them on this computer."
          />
        )}
      </div>
    </>
  );
}
