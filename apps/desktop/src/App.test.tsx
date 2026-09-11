import { render, screen } from "@testing-library/react";
import { vi, test, expect } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue("pong"),
}));

import App from "./App";

test("renders the ping result from magi-core", async () => {
  render(<App />);
  expect(await screen.findByTestId("ping-result")).toHaveTextContent(
    "magi-core says: pong",
  );
});
