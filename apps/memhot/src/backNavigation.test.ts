import { describe, expect, it } from "vitest";
import { createBackNavigation } from "./backNavigation";

describe("screen back navigation", () => {
  it("closes one inner screen at a time before returning to the root", () => {
    const back = createBackNavigation();
    const closed: string[] = [];
    let removeReport: () => void;
    let removeMaterial: () => void;
    removeReport = back.register(0, () => { closed.push("report"); removeReport(); });
    removeMaterial = back.register(10, () => { closed.push("material"); removeMaterial(); });
    expect(back.dispatch()).toBe(true);
    expect(closed).toEqual(["material"]);
    expect(back.dispatch()).toBe(true);
    expect(closed).toEqual(["material", "report"]);
    expect(back.dispatch()).toBe(false);
  });
  it("keeps an open editor ahead of its parent regardless of effect registration order", () => {
    const back = createBackNavigation();
    const closed: string[] = [];
    back.register(20, () => closed.push("editor"));
    back.register(0, () => closed.push("page"));
    back.dispatch();
    expect(closed).toEqual(["editor"]);
  });
  it("unregistered hidden screens do not intercept a later screen", () => {
    const back = createBackNavigation();
    const remove = back.register(30, () => { throw new Error("Hidden screen received back"); });
    remove();
    remove();
    expect(back.dispatch()).toBe(false);
  });
  it("a busy editor consumes back without exiting the application", () => {
    const back = createBackNavigation();
    let exited = false;
    back.register(0, () => { exited = true; });
    back.register(20, () => {});
    expect(back.dispatch()).toBe(true);
    expect(exited).toBe(false);
  });
});
