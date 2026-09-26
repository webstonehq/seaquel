import type { QueryParameter } from "$lib/types";
import { extractParameters, createDefaultParameters } from "$lib/sql";
import type { QueryEditorContext } from "./types.js";

export type PendingAction =
  | "query"
  | { type: "query-current"; cursorOffset: number }
  | { type: "explain"; analyze: boolean; cursorOffset: number }
  | { type: "visualize"; cursorOffset: number }
  | null;

export function createParamDialog(ctx: QueryEditorContext) {
  let show = $state(false);
  let params = $state<QueryParameter[]>([]);
  let action = $state<PendingAction>(null);

  function getParameterDefinitions(query: string): QueryParameter[] {
    const activeTab = ctx.getActiveTab();
    const queryId = activeTab?.queryId;
    const savedQuery = queryId ? ctx.db.state.projectQueries.find((q) => q.id === queryId) : null;

    if (savedQuery?.parameters && savedQuery.parameters.length > 0) {
      return savedQuery.parameters;
    }

    const paramNames = extractParameters(query);
    return createDefaultParameters(paramNames);
  }

  return {
    get show() {
      return show;
    },
    set show(v: boolean) {
      show = v;
    },
    get params() {
      return params;
    },
    set params(v: QueryParameter[]) {
      params = v;
    },
    get action() {
      return action;
    },
    set action(v: PendingAction) {
      action = v;
    },
    getParameterDefinitions,
  };
}

export type ParamDialog = ReturnType<typeof createParamDialog>;
