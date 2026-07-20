// @vitest-environment happy-dom

import ui from "@nuxt/ui/vue-plugin";
import "virtual:nuxt-icon-bundle/register";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, reactive, type App as VueApp } from "vue";
import type { ExerciseSnapshot } from "../types/learning";
import ExerciseSidebar from "./ExerciseSidebar.vue";

const mountedApps: VueApp[] = [];

type ExerciseFixture = Omit<ExerciseSnapshot, "solutionPath" | "solutionAvailable">;

function exerciseFixtures(exercises: ExerciseFixture[]): ExerciseSnapshot[] {
  return exercises.map((exercise) => ({
    ...exercise,
    solutionPath: exercise.sourcePath.replace(/^exercises\//, "solutions/"),
    solutionAvailable: exercise.status === "completed" || exercise.id === "variables2",
  }));
}

const exercises = exerciseFixtures([
  {
    id: "intro1",
    sourcePath: "exercises/00_intro/intro1.rs",
    status: "completed",
    revision: 1,
  },
  {
    id: "variables1",
    sourcePath: "exercises/01_variables/variables1.rs",
    status: "completed",
    revision: 1,
  },
  {
    id: "variables2",
    sourcePath: "exercises/01_variables/variables2.rs",
    status: "current",
    revision: 1,
  },
  {
    id: "functions1",
    sourcePath: "exercises/02_functions/functions1.rs",
    status: "unlocked",
    revision: 0,
  },
  {
    id: "quiz1",
    sourcePath: "exercises/quizzes/quiz1.rs",
    status: "locked",
    revision: 0,
  },
  {
    id: "primitive_types1",
    sourcePath: "exercises/04_primitive_types/primitive_types1.rs",
    status: "locked",
    revision: 0,
  },
  {
    id: "quiz2",
    sourcePath: "exercises/quizzes/quiz2.rs",
    status: "locked",
    revision: 0,
  },
  {
    id: "move_semantics1",
    sourcePath: "exercises/06_move_semantics/move_semantics1.rs",
    status: "locked",
    revision: 0,
  },
  {
    id: "quiz3",
    sourcePath: "exercises/quizzes/quiz3.rs",
    status: "locked",
    revision: 0,
  },
]);

function button(host: HTMLElement, name: string) {
  return [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) => {
    const label = item.getAttribute("aria-label");
    return label === name || label?.startsWith(`${name},`);
  });
}

function visibleText(host: HTMLElement) {
  return host.textContent ?? "";
}

async function mount(overrides: Record<string, unknown> = {}) {
  const host = document.createElement("div");
  document.body.append(host);
  const props = reactive({
    exercises,
    selected: "variables2",
    selectedSolution: undefined as string | undefined,
    disabled: false,
    dirty: false,
    saving: false,
    saveError: undefined as string | undefined,
    ...overrides,
  });
  const selected: string[] = [];
  const selectedSolutions: string[] = [];
  const app = createApp({
    setup: () => () =>
      h(ExerciseSidebar, {
        ...props,
        onSelect: (exerciseId: string) => selected.push(exerciseId),
        onSelectSolution: (exerciseId: string) => selectedSolutions.push(exerciseId),
      }),
  }).use(ui);
  mountedApps.push(app);
  app.mount(host);
  await nextTick();
  return { host, props, selected, selectedSolutions };
}

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  document.body.replaceChildren();
});

describe("ExerciseSidebar", () => {
  it("initially expands Exercises and only the selected folder, then keeps multiple folders open", async () => {
    const { host } = await mount();

    expect(button(host, "Exercises folder")?.getAttribute("aria-expanded")).toBe("true");
    expect(button(host, "01_variables folder")?.getAttribute("aria-expanded")).toBe("true");
    expect(visibleText(host)).toContain("variables2.rs");
    expect(visibleText(host)).not.toContain("intro1.rs");

    button(host, "00_intro folder")?.click();
    await nextTick();
    expect(visibleText(host)).toContain("intro1.rs");
    expect(visibleText(host)).toContain("variables2.rs");
  });

  it("reveals a newly selected exercise without closing manually expanded folders", async () => {
    const { host, props } = await mount();
    button(host, "00_intro folder")?.click();
    props.selected = "functions1";
    await nextTick();

    expect(visibleText(host)).toContain("intro1.rs");
    expect(visibleText(host)).toContain("functions1.rs");
    expect(button(host, "02_functions folder")?.getAttribute("aria-expanded")).toBe("true");
  });

  it("uses audited path labels and groups quizzes at their first curriculum occurrence", async () => {
    const { host } = await mount();
    const root = host.querySelector('[data-folder-path="exercises"]')!;
    const labels = [...root.querySelectorAll(":scope > ul > li > button")].map((item) =>
      item.textContent?.trim(),
    );

    expect(labels).toEqual([
      expect.stringContaining("00_intro"),
      expect.stringContaining("01_variables"),
      expect.stringContaining("02_functions"),
      expect.stringContaining("quizzes"),
      expect.stringContaining("04_primitive_types"),
      expect.stringContaining("06_move_semantics"),
    ]);
    button(host, "quizzes folder")?.click();
    await nextTick();
    expect(visibleText(host)).toContain("quiz1.rs");
    expect(visibleText(host)).toContain("quiz2.rs");
    expect(visibleText(host)).toContain("quiz3.rs");
    expect(visibleText(host)).not.toContain("README");
  });

  it("searches full displayed paths, shows no results, and restores the exact expansion set", async () => {
    const { host } = await mount();
    button(host, "00_intro folder")?.click();
    await nextTick();
    const search = host.querySelector<HTMLInputElement>('input[type="search"]')!;

    search.value = "EXERCISES/06_MOVE_SEMANTICS/MOVE_SEMANTICS1.RS";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(visibleText(host)).toContain("move_semantics1.rs");
    expect(visibleText(host)).not.toContain("variables2.rs");

    search.value = "not/a/real/path";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(host.querySelector('[role="status"]')?.textContent).toContain("No files found");

    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(visibleText(host)).toContain("intro1.rs");
    expect(visibleText(host)).toContain("variables2.rs");
    expect(visibleText(host)).not.toContain("functions1.rs");
  });

  it("restores collapsed selected ancestors when search clears without navigation", async () => {
    const { host } = await mount();
    button(host, "01_variables folder")?.click();
    button(host, "Exercises folder")?.click();
    const search = host.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "functions1.rs";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();

    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(button(host, "Exercises folder")?.getAttribute("aria-expanded")).toBe("false");

    button(host, "Exercises folder")?.click();
    await nextTick();
    expect(button(host, "01_variables folder")?.getAttribute("aria-expanded")).toBe("false");
  });

  it("reveals a selection made during search after restoring prior expansion", async () => {
    const { host, props } = await mount();
    button(host, "00_intro folder")?.click();
    const search = host.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "move_semantics1.rs";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    props.selected = "move_semantics1";
    await nextTick();

    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();

    expect(visibleText(host)).toContain("intro1.rs");
    expect(visibleText(host)).toContain("variables2.rs");
    expect(visibleText(host)).toContain("move_semantics1.rs");
  });

  it("renders Nuxt UI Lucide icons for disclosure and exercise status", async () => {
    const { host } = await mount({
      exercises: exercises.map((exercise) =>
        exercise.id === "variables2" ? { ...exercise, solutionAvailable: false } : exercise,
      ),
    });
    button(host, "02_functions folder")?.click();
    button(host, "quizzes folder")?.click();
    await nextTick();

    expect(host.querySelectorAll(".iconify--lucide")).toHaveLength(
      host.querySelectorAll("button").length,
    );
    expect(button(host, "Exercises folder")?.querySelector("path")?.getAttribute("d")).toBe(
      "m6 9l6 6l6-6",
    );
    expect(button(host, "00_intro folder")?.querySelector("path")?.getAttribute("d")).toBe(
      "m9 18l6-6l-6-6",
    );
    expect(button(host, "variables1.rs, Completed")?.querySelectorAll("circle")).toHaveLength(1);
    expect(button(host, "variables1.rs, Completed")?.querySelector("path")).not.toBeNull();
    expect(button(host, "variables2.rs, Current")?.querySelectorAll("circle")).toHaveLength(2);
    expect(button(host, "functions1.rs, Available")?.querySelectorAll("circle")).toHaveLength(1);
    expect(button(host, "functions1.rs, Available")?.querySelector("path, rect")).toBeNull();
    expect(button(host, "quiz1.rs, Locked")?.querySelector("rect")).not.toBeNull();
  });

  it("shows completion counts and accessible statuses while locked leaves cannot select", async () => {
    const { host, selected } = await mount();

    expect(button(host, "01_variables folder")?.textContent).toContain("2/2 completed");
    expect(button(host, "variables2.rs, Completed")?.getAttribute("aria-current")).toBe("step");
    button(host, "02_functions folder")?.click();
    button(host, "quizzes folder")?.click();
    await nextTick();
    button(host, "functions1.rs, Available")?.click();
    button(host, "quiz1.rs, Locked")?.click();

    expect(button(host, "quiz1.rs, Locked")?.disabled).toBe(true);
    expect(selected).toEqual(["functions1"]);
  });

  it("shows every solution path but enables only proof-authorized leaves", async () => {
    const { host, selectedSolutions } = await mount();
    expect(button(host, "Solutions folder")?.getAttribute("aria-expanded")).toBe("false");

    button(host, "Solutions folder")?.click();
    await nextTick();
    button(host, "00_intro solutions folder")?.click();
    button(host, "quizzes solutions folder")?.click();
    await nextTick();

    const available = button(host, "intro1.rs, Solution available");
    const locked = button(host, "quiz1.rs, Solution locked");
    available?.click();
    locked?.click();

    expect(available?.disabled).toBe(false);
    expect(locked?.disabled).toBe(true);
    expect(selectedSolutions).toEqual(["intro1"]);
  });

  it("searches solution full paths without persisting the Solutions expansion", async () => {
    const { host } = await mount();
    const search = host.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "SOLUTIONS/06_MOVE_SEMANTICS/MOVE_SEMANTICS1.RS";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(visibleText(host)).toContain("move_semantics1.rs");

    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(button(host, "Solutions folder")?.getAttribute("aria-expanded")).toBe("false");
  });

  it("keeps save state in a fixed footer outside the scrolling tree", async () => {
    const { host, props } = await mount();
    const footer = host.querySelector("footer")!;
    expect(footer.textContent).toContain("Saved");
    expect(footer.previousElementSibling?.classList.contains("exercise-tree-scroll")).toBe(true);

    props.saving = true;
    await nextTick();
    expect(footer.textContent).toContain("Saving…");
    props.saving = false;
    props.dirty = true;
    await nextTick();
    expect(footer.textContent).toContain("Unsaved");
    props.saveError = "disk full";
    await nextTick();
    expect(footer.textContent).toContain("Save failed: disk full");
  });
});
