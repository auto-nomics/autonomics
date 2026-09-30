import { describe, expect, test } from "bun:test";
import { renderToString } from "react-dom/server";

import { App } from "@/app";

describe("bibliography frontend", () => {
  test("renders the shadcn workbench shell", () => {
    const html = renderToString(<App />);
    expect(html).toContain("Collections");
    expect(html).toContain("Records");
    expect(html).toContain("Select a record to inspect");
  });
});
