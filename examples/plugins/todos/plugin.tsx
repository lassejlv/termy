/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */

type Todo = {
  id: string;
  title: string;
  done: boolean;
};

const STORAGE_KEY = "todos";
const PAGE_KEY = "todos-page";
const PAGE_SIZE = 24;

async function loadTodos(context: TermyPluginContext): Promise<Todo[]> {
  return (await context.storage.get<Todo[]>(STORAGE_KEY)) ?? [];
}

export default definePlugin({
  commands: [
    {
      id: "open",
      title: "Todos: Open",
      keywords: ["tasks", "checklist"],
      icon: "info",
      run() {
        return { type: "view.open", view: "todos" };
      },
    },
    {
      id: "open-palette",
      title: "Todos: Open in Command Palette",
      keywords: ["tasks", "checklist", "palette"],
      icon: "command",
      run() {
        return {
          type: "view.open",
          view: "todos",
          target: "commandPalette",
        };
      },
    },
  ],

  views: {
    todos: {
      title: "Todos",

      async render({ context }) {
        const todos = await loadTodos(context);
        const pageCount = Math.max(1, Math.ceil(todos.length / PAGE_SIZE));
        const storedPage = (await context.storage.get<number>(PAGE_KEY)) ?? 0;
        const page = Math.max(0, Math.min(storedPage, pageCount - 1));
        const visibleTodos = todos.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

        return (
          <TermyUI.Column gap="medium">
            <TermyUI.Row gap="small" align="center">
              <TermyUI.TextInput
                id="title"
                placeholder="Add a task…"
                submit="add"
              />
              <TermyUI.Button id="add-button" action="add" variant="primary">
                Add
              </TermyUI.Button>
            </TermyUI.Row>

            <TermyUI.Divider />

            {todos.length === 0 ? (
              <TermyUI.Text tone="muted">Nothing to do. Suspicious.</TermyUI.Text>
            ) : (
              visibleTodos.map((todo) => (
                <TermyUI.Row key={todo.id} gap="small" align="center">
                  <TermyUI.Checkbox
                    id={`todo-${todo.id}`}
                    action="toggle"
                    payload={todo.id}
                    checked={todo.done}
                  >
                    {todo.title}
                  </TermyUI.Checkbox>
                  <TermyUI.Button
                    id={`delete-${todo.id}`}
                    action="delete"
                    payload={todo.id}
                    variant="danger"
                  >
                    Delete
                  </TermyUI.Button>
                </TermyUI.Row>
              ))
            )}

            {pageCount > 1 ? (
              <TermyUI.Row gap="small" align="center">
                <TermyUI.Button
                  id="previous-page"
                  action="previous-page"
                  disabled={page === 0}
                >
                  Previous
                </TermyUI.Button>
                <TermyUI.Text tone="muted">
                  Page {page + 1} of {pageCount}
                </TermyUI.Text>
                <TermyUI.Button
                  id="next-page"
                  action="next-page"
                  disabled={page + 1 >= pageCount}
                >
                  Next
                </TermyUI.Button>
              </TermyUI.Row>
            ) : null}
          </TermyUI.Column>
        );
      },

      async onAction({ action, values, context }) {
        const todos = await loadTodos(context);
        const pageCount = Math.max(1, Math.ceil(todos.length / PAGE_SIZE));
        const storedPage = (await context.storage.get<number>(PAGE_KEY)) ?? 0;
        const page = Math.max(0, Math.min(storedPage, pageCount - 1));

        if (action.id === "add") {
          const title = String(values.title ?? "").trim();
          if (!title) {
            context.toasts.info("Give the todo a title first");
            return;
          }
          await context.storage.set(STORAGE_KEY, [
            ...todos,
            { id: crypto.randomUUID(), title, done: false },
          ]);
          await context.storage.set(PAGE_KEY, Math.floor(todos.length / PAGE_SIZE));
          return;
        }

        if (action.id === "previous-page") {
          await context.storage.set(PAGE_KEY, Math.max(0, page - 1));
          return;
        }
        if (action.id === "next-page") {
          await context.storage.set(PAGE_KEY, Math.min(pageCount - 1, page + 1));
          return;
        }

        if (!action.payload) return;
        if (action.id === "toggle") {
          await context.storage.set(
            STORAGE_KEY,
            todos.map((todo) =>
              todo.id === action.payload ? { ...todo, done: !todo.done } : todo,
            ),
          );
        }
        if (action.id === "delete") {
          const remaining = todos.filter((todo) => todo.id !== action.payload);
          await context.storage.set(STORAGE_KEY, remaining);
          const remainingPageCount = Math.max(1, Math.ceil(remaining.length / PAGE_SIZE));
          await context.storage.set(PAGE_KEY, Math.min(page, remainingPageCount - 1));
        }
      },
    },
  },
} satisfies TermyPlugin);
