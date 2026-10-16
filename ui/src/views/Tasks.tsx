// The Now list (9 · Brain): a frosted panel at the right with a project's open tasks (or every
// project's) and the latest notes. A task from today's session carries a signal dot. Sections
// rise in one after the other, as on the canvas.

import type { View } from "../protocol/generated/View.ts";
import { noteMeta, taskMeta } from "./labels.ts";
import { TaskCircle } from "./parts.tsx";

export function TasksView({
  view,
  today,
}: {
  view: Extract<View, { kind: "Tasks" }>;
  today: string;
}) {
  return (
    <section class="panel panel-tasks" aria-label="Brain">
      <div class="panel-head">
        <span class="panel-label">Brain</span>
        <h2 class="panel-title">{view.project ?? "Now"}</h2>
      </div>
      <div class="panel-section" data-stagger="0">
        <span class="panel-section-title">{view.project === null ? "Open tasks" : "Now"}</span>
        {view.tasks.length === 0 ? (
          <p class="panel-empty">Nothing open.</p>
        ) : (
          <div class="panel-rows">
            {view.tasks.map((task) => (
              <div key={task.id} class="task-row">
                <TaskCircle />
                <span class="task-title">{task.title}</span>
                {task.from !== null && task.from.date === today ? (
                  <span class="task-new" role="img" aria-label="new today" />
                ) : null}
                <span class="row-meta">{taskMeta(task, today)}</span>
              </div>
            ))}
          </div>
        )}
        {view.more_tasks > 0 ? <span class="row-more">{view.more_tasks} more</span> : null}
      </div>
      {view.notes.length > 0 ? (
        <div class="panel-section" data-stagger="1">
          <span class="panel-section-title">Recent notes</span>
          <div class="panel-rows">
            {view.notes.map((note) => (
              <div key={note.id} class="note-row">
                <span class="note-text">{note.text}</span>
                <span class="row-meta">{noteMeta(note, today)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
}
