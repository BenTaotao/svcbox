import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { call, errorText } from "../api";
import type {
  InstanceView,
  PathsInfo,
  Settings,
  VersionInfo,
} from "../types";
import { ConfirmModal, type ConfirmOptions, type ConfirmState } from "../components/ConfirmModal";
import { ToastStack, type ToastItem, type ToastType } from "../components/Toast";

export type Page = "dashboard" | "versions" | "certs" | "settings" | "instance";

interface StoreValue {
  instances: InstanceView[];
  versions: VersionInfo[];
  settings: Settings | null;
  paths: PathsInfo | null;
  loading: boolean;
  page: Page;
  selectedId: string | null;
  navigate: (page: Page) => void;
  openInstance: (id: string) => void;
  refreshInstances: (silent?: boolean) => Promise<void>;
  refreshVersions: () => Promise<void>;
  reloadSettings: () => Promise<void>;
  refreshAll: () => Promise<void>;
  toast: (message: string, type?: ToastType) => void;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
  runAction: <T>(
    key: string,
    action: () => Promise<T>,
    successMessage?: string,
  ) => Promise<T | undefined>;
  isPending: (key: string) => boolean;
}

const StoreContext = createContext<StoreValue | null>(null);

export function useApp(): StoreValue {
  const value = useContext(StoreContext);
  if (!value) throw new Error("useApp 必须在 AppStoreProvider 内使用");
  return value;
}

export function AppStoreProvider({ children }: { children: React.ReactNode }) {
  const [instances, setInstances] = useState<InstanceView[]>([]);
  const [versions, setVersions] = useState<VersionInfo[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [paths, setPaths] = useState<PathsInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState<Page>("dashboard");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [confirmState, setConfirmState] = useState<ConfirmState | null>(null);
  const [pending, setPending] = useState<Set<string>>(new Set());
  const toastId = useRef(0);

  const toast = useCallback((message: string, type: ToastType = "info") => {
    const id = ++toastId.current;
    setToasts((prev) => [...prev, { id, message, type }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4500);
  }, []);

  const refreshInstances = useCallback(
    async (silent = false) => {
      try {
        const list = await call<InstanceView[]>("list_instances");
        setInstances(list);
      } catch (e) {
        if (!silent) toast(errorText(e), "error");
      } finally {
        setLoading(false);
      }
    },
    [toast],
  );

  const refreshVersions = useCallback(async () => {
    try {
      setVersions(await call<VersionInfo[]>("list_versions"));
    } catch (e) {
      toast(errorText(e), "error");
    }
  }, [toast]);

  const reloadSettings = useCallback(async () => {
    try {
      const [s, p] = await Promise.all([
        call<Settings>("get_settings"),
        call<PathsInfo>("get_paths"),
      ]);
      setSettings(s);
      setPaths(p);
    } catch (e) {
      toast(errorText(e), "error");
    }
  }, [toast]);

  const refreshAll = useCallback(async () => {
    setLoading(true);
    await Promise.all([refreshInstances(true), refreshVersions(), reloadSettings()]);
    setLoading(false);
  }, [refreshInstances, refreshVersions, reloadSettings]);

  const navigate = useCallback((next: Page) => {
    setPage(next);
    if (next !== "instance") setSelectedId(null);
  }, []);

  const openInstance = useCallback((id: string) => {
    setSelectedId(id);
    setPage("instance");
  }, []);

  const confirm = useCallback(
    (options: ConfirmOptions) =>
      new Promise<boolean>((resolve) => {
        setConfirmState({
          ...options,
          resolve: (ok) => {
            setConfirmState(null);
            resolve(ok);
          },
        });
      }),
    [],
  );

  const runAction = useCallback(
    async <T,>(
      key: string,
      action: () => Promise<T>,
      successMessage?: string,
    ): Promise<T | undefined> => {
      setPending((prev) => new Set(prev).add(key));
      try {
        const result = await action();
        if (successMessage) toast(successMessage, "success");
        return result;
      } catch (e) {
        toast(errorText(e), "error");
        return undefined;
      } finally {
        setPending((prev) => {
          const next = new Set(prev);
          next.delete(key);
          return next;
        });
      }
    },
    [toast],
  );

  const isPending = useCallback((key: string) => pending.has(key), [pending]);

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const setup = async () => {
      const keep = (unlisten: () => void) => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      };
      const reload = () => {
        if (!disposed) void refreshInstances(true);
      };
      keep(await listen("instance-changed", reload));
      keep(await listen("nginx-sites-changed", reload));
      keep(await listen("mysql-changed", reload));
      keep(
        await listen("version-changed", () => {
          if (!disposed) void refreshVersions();
        }),
      );
      keep(
        await listen("settings-changed", () => {
          if (!disposed) void reloadSettings();
        }),
      );
    };
    void setup();
    const timer = window.setInterval(() => {
      if (!disposed) void refreshInstances(true);
    }, 5000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [refreshInstances, refreshVersions, reloadSettings]);

  const value = useMemo<StoreValue>(
    () => ({
      instances,
      versions,
      settings,
      paths,
      loading,
      page,
      selectedId,
      navigate,
      openInstance,
      refreshInstances,
      refreshVersions,
      reloadSettings,
      refreshAll,
      toast,
      confirm,
      runAction,
      isPending,
    }),
    [
      instances,
      versions,
      settings,
      paths,
      loading,
      page,
      selectedId,
      navigate,
      openInstance,
      refreshInstances,
      refreshVersions,
      reloadSettings,
      refreshAll,
      toast,
      confirm,
      runAction,
      isPending,
    ],
  );

  return (
    <StoreContext.Provider value={value}>
      {children}
      <ToastStack
        toasts={toasts}
        onDismiss={(id) => setToasts((prev) => prev.filter((t) => t.id !== id))}
      />
      <ConfirmModal state={confirmState} />
    </StoreContext.Provider>
  );
}
