import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { features } from ".";

describe("features", () => {
  it("lists the seven features once each, pairing first", () => {
    const ids = features.map((f) => f.id);
    expect(ids).toEqual([
      "pairing",
      "notifications",
      "calls",
      "photos",
      "files",
      "clipboard",
      "screen",
    ]);
  });

  it.each(
    features.filter(
      (f) => !["pairing", "notifications", "calls", "clipboard", "files"].includes(f.id),
    ),
  )(
    "$label renders a placeholder page that says what it will do",
    ({ Page, label, description }) => {
      render(<Page />);

      expect(screen.getByRole("heading", { level: 1, name: label })).toBeInTheDocument();
      expect(screen.getByText(description)).toBeInTheDocument();
      expect(screen.getByText("In development")).toBeInTheDocument();
      expect(screen.getByRole("region")).toHaveTextContent("still in development");
    },
  );
});
