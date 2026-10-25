import { Info } from "lucide-react";
import { useEffect, useId, useRef } from "react";

import { Button, Callout, Card } from "@/components/ui";

/** How to pair, shown after the user starts pairing. */
export function PairingSteps({ onCancel }: { onCancel: () => void }) {
  const titleId = useId();
  const titleRef = useRef<HTMLHeadingElement>(null);

  // The button that opened this panel is gone, so move focus to its title.
  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  return (
    <Card className="pairing-steps">
      <section aria-labelledby={titleId}>
        <h2 id={titleId} ref={titleRef} tabIndex={-1} className="pairing-steps__title">
          Pair a new phone
        </h2>
        <ol className="pairing-steps__list">
          <li>Install the link2linux app on your Android phone.</li>
          <li>Connect the phone and this computer to the same network.</li>
          <li>Open the app on the phone and choose this computer from the list.</li>
          <li>Check that both screens show the same code, then confirm on both.</li>
        </ol>

        <div className="pairing-steps__code" aria-hidden="true">
          <span>– – –</span>
          <span>– – –</span>
        </div>

        <Callout>
          <Info aria-hidden="true" />
          <p>Pairing isn&apos;t available in this build yet. These steps show how it will work.</p>
        </Callout>

        <div className="pairing-steps__actions">
          <Button onClick={onCancel}>Cancel</Button>
        </div>
      </section>
    </Card>
  );
}
