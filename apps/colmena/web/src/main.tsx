import { StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { Scene } from "./scene/Scene";
import { Hud } from "./Hud";
import { TourPanel } from "./TourPanel";
import { useStore } from "./store";
import "./styles.css";

function App() {
  useEffect(() => useStore.getState().init(), []);
  return (
    <>
      <Scene />
      <Hud />
      <TourPanel />
    </>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
if (import.meta.env.DEV) (window as unknown as { __store: typeof useStore }).__store = useStore;
