// Bridges the existing zustand vanilla store into SolidJS reactivity without
// rewriting the reducer. zustand exposes `getState` + `subscribe`; we seed a
// Solid signal from the current snapshot and push every store mutation into it.
// Solid's fine-grained reactivity then re-renders only the components that read
// the slices that changed (via the selector accessors below).

import { createSignal, onCleanup, type Accessor } from "solid-js";

import { useStore, type State } from "../store/index.js";

/// Returns a Solid accessor that yields the latest full store snapshot and
/// re-renders subscribers whenever `applyEvent` (or any setter) mutates state.
export function useStoreSnapshot(): Accessor<State> {
  const [snapshot, setSnapshot] = createSignal<State>(useStore.getState());
  const unsubscribe = useStore.subscribe((state) => setSnapshot(() => state));
  onCleanup(unsubscribe);
  return snapshot;
}

/// Derives a memo-like accessor for one slice of the store. The selector runs
/// on every store change but the returned accessor only triggers downstream
/// updates when the selected value is no longer `Object.is`-equal.
export function useStoreSelector<T>(selector: (state: State) => T): Accessor<T> {
  const [value, setValue] = createSignal<T>(selector(useStore.getState()));
  const unsubscribe = useStore.subscribe((state) => {
    const next = selector(state);
    setValue((prev) => (Object.is(prev, next) ? prev : next) as T);
  });
  onCleanup(unsubscribe);
  return value;
}
