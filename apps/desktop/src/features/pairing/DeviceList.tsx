import { Smartphone } from "lucide-react";

import { Badge, Card } from "@/components/ui";
import type { Device } from "@/lib/api";

/** The phones paired with this computer. */
export function DeviceList({ devices }: { devices: Device[] }) {
  return (
    <ul className="device-list" aria-label="Paired phones">
      {devices.map((device) => (
        <li key={device.id}>
          <Card className="device-list__item">
            <Smartphone className="device-list__icon" aria-hidden="true" />
            <div className="device-list__text">
              <p className="device-list__name">{device.name}</p>
              {device.model && <p className="device-list__model">{device.model}</p>}
            </div>
            {device.status === "connected" ? (
              <Badge tone="success" dot>
                Connected
              </Badge>
            ) : (
              <Badge dot>Not connected</Badge>
            )}
          </Card>
        </li>
      ))}
    </ul>
  );
}
