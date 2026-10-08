import { useEffect, useRef } from "react";
import { endTour, goto, startTour, STEPS, tourSeen } from "./tour";
import { useStore } from "./store";

export function TourPanel() {
  const tour = useStore((s) => s.tour);
  const ready = useStore((s) => s.docs.length > 0);

  const tried = useRef(false);
  useEffect(() => {
    if (!ready || tried.current) return;
    tried.current = true; // solo en la primera carga; no al repoblar tras vaciar la colmena
    if (!tourSeen() && !useStore.getState().lab) startTour();
  }, [ready]);

  if (tour === null) {
    return (
      <button className="tour-btn" onClick={startTour}>
        ▶ Tour guiado
      </button>
    );
  }
  const s = STEPS[tour]!;
  return (
    <section className="panel tour">
      <h3>Paso {tour + 1} de {STEPS.length}</h3>
      <h2>{s.title}</h2>
      <p>{s.body}</p>
      <div className="tour-nav">
        <button className="ghost" onClick={endTour}>Saltar</button>
        <span />
        <button className="ghost" disabled={tour === 0} onClick={() => goto(tour - 1)}>←</button>
        <button onClick={() => goto(tour + 1)}>{tour === STEPS.length - 1 ? "Empezar a explorar" : "Siguiente →"}</button>
      </div>
      <div className="tour-dots">
        {STEPS.map((_, i) => (
          <i key={i} className={i === tour ? "on" : ""} />
        ))}
      </div>
    </section>
  );
}
