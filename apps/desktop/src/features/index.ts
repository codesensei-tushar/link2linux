import { CallsPage } from "./calls/CallsPage";
import { callsMeta } from "./calls/meta";
import { ClipboardPage } from "./clipboard/ClipboardPage";
import { clipboardMeta } from "./clipboard/meta";
import { FilesPage } from "./files/FilesPage";
import { filesMeta } from "./files/meta";
import { messagesMeta } from "./messages/meta";
import { MessagesPage } from "./messages/MessagesPage";
import { notificationsMeta } from "./notifications/meta";
import { NotificationsPage } from "./notifications/NotificationsPage";
import { pairingMeta } from "./pairing/meta";
import { PairingPage } from "./pairing/PairingPage";
import { photosMeta } from "./photos/meta";
import { PhotosPage } from "./photos/PhotosPage";
import { screenMeta } from "./screen/meta";
import { ScreenPage } from "./screen/ScreenPage";
import type { Feature } from "./types";

export type { Feature, FeatureId, FeatureMeta } from "./types";

/** Every feature, in navigation order. Pairing comes first: nothing works without it. */
export const features: readonly Feature[] = [
  { ...pairingMeta, Page: PairingPage },
  { ...notificationsMeta, Page: NotificationsPage },
  { ...messagesMeta, Page: MessagesPage },
  { ...callsMeta, Page: CallsPage },
  { ...photosMeta, Page: PhotosPage },
  { ...filesMeta, Page: FilesPage },
  { ...clipboardMeta, Page: ClipboardPage },
  { ...screenMeta, Page: ScreenPage },
];
