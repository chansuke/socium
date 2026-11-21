import type { Activity, Plan, PlanTaskStatus } from "./protocol.ts";

/** One line of what the agent is doing, for a status line beside the transcript. */
export function describeActivity(activity: Activity | null): string {
  const detail = activity?.detail;
  switch (activity?.phase) {
    case "requesting":
      return detail ? `Waiting for ${detail}…` : "Waiting for the model…";
    case "thinking":
      return "Thinking…";
    case "responding":
      return "Writing…";
    case "tool":
      return detail ? `Using ${detail}…` : "Using a tool…";
    case "command":
      return detail ? `Running ${detail}…` : "Running a command…";
    case "approval":
      return "Waiting for approval…";
    case "compacting":
      return "Compacting the conversation…";
    default:
      return "Running…";
  }
}

export const planStatusLabels: Record<PlanTaskStatus, string> = {
  done: "Done",
  active: "In progress",
  pending: "Pending",
  blocked: "Blocked",
  dropped: "Dropped",
};

/** Counts the plan's leaf tasks; groups take their status from them. */
export function planProgress(plan: Plan) {
  const tasks = plan.tasks.filter(({ group, status }) => !group && status !== "dropped");
  return {
    done: tasks.filter(({ status }) => status === "done").length,
    total: tasks.length,
    active: plan.tasks.find(({ group, status }) => !group && status === "active") ?? null,
    percent: Math.round(plan.done / 10),
  };
}
