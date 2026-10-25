import { render, screen } from "@testing-library/react";
import { Smartphone } from "lucide-react";
import { describe, expect, it } from "vitest";

import { Button } from "./Button";
import { EmptyState } from "./EmptyState";
import { Spinner } from "./Spinner";

describe("EmptyState", () => {
  it("is a region named by its title, with an h2 by default", () => {
    render(
      <EmptyState
        icon={Smartphone}
        title="Nothing here yet"
        description="Add something to get started."
        actions={<Button>Add</Button>}
      />,
    );

    const region = screen.getByRole("region", { name: "Nothing here yet" });
    expect(screen.getByRole("heading", { level: 2, name: "Nothing here yet" })).toBeVisible();
    expect(region).toHaveTextContent("Add something to get started.");
    expect(screen.getByRole("button", { name: "Add" })).toBeInTheDocument();
  });

  it("supports a lower heading level for nested use", () => {
    render(<EmptyState icon={Smartphone} title="Empty" description="-" headingLevel={3} />);
    expect(screen.getByRole("heading", { level: 3, name: "Empty" })).toBeInTheDocument();
  });
});

describe("Spinner", () => {
  it("announces its label even when the label is visually hidden", () => {
    render(<Spinner label="Loading devices" hideLabel />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading devices");
  });
});
