import { createTRPCReact } from "@trpc/react-query";
import { createTRPCProxyClient, type TRPCLink } from "@trpc/client";
import { observable } from "@trpc/server/observable";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import superjson from "superjson";
import type { AppRouter } from "./router";

const isTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI__" in window;

const tauriLink = (): TRPCLink<AppRouter> => () => ({ op }) =>
  observable((observer) => {
    if (!isTauriRuntime()) {
      observer.error(
        new Error(
          `[tauri] tRPC is unavailable outside the Tauri runtime (${op.path})`,
        ),
      );
      return;
    }

    if (op.type === "subscription") {
      const eventName = `trpc:${op.path}`;
      const unlisten = listen(eventName, (event) => {
        observer.next({ result: { data: event.payload } });
      }).catch((error) => {
        observer.error(error);
        return () => {};
      });
      return () => {
        void unlisten.then((stop) => stop());
      };
    }

    const input = superjson.serialize(op.input);

    void invoke("trpc", {
      path: op.path,
      type: op.type,
      input,
    })
      .then((data) => {
        const payload = data as any;
        const output =
          payload && typeof payload === "object" && "json" in payload
            ? superjson.deserialize(payload)
            : payload;
        observer.next({ result: { data: output } });
        observer.complete();
      })
      .catch((error) => observer.error(error));
  });

// Create the tRPC React hooks
export const api = createTRPCReact<AppRouter>();

// Create the vanilla tRPC client (for use outside React components)
export const trpcClient = createTRPCProxyClient<AppRouter>({
  transformer: superjson,
  links: [tauriLink()],
});
