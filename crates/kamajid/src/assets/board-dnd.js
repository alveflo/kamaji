// kamaji — board drag-and-drop column moves.
//
// Cards are `draggable` (views/card.rs); each `.col-body` is a drop zone. On a
// drop we POST /tickets/:id/move {target} via fetch() — the existing JSON
// command API is the writer — and the authoritative re-render (cards relocated,
// column counts recounted) arrives over the SSE stream (/ui/events). We never
// move the DOM ourselves; SSE is the source of truth.
//
// All listeners are delegated on `document`, so they keep working after Datastar
// morph-patches a column/card in place (a per-element listener would be lost on
// the next live patch). Dragging into Done is a status change only — the `✓ Done`
// button remains the path that tears the session down.
(() => {
  let dragged = null; // the .card being dragged
  let dropZone = null; // the .col-body currently highlighted

  const colBodyOf = (el) => (el && el.closest ? el.closest(".col-body") : null);
  const statusOf = (body) => {
    const col = body && body.closest(".column");
    return col ? col.dataset.status : null;
  };
  const clearDrop = () => {
    document
      .querySelectorAll(".col-body.drop")
      .forEach((b) => b.classList.remove("drop"));
    dropZone = null;
  };

  document.addEventListener("dragstart", (e) => {
    const card = e.target.closest && e.target.closest(".card");
    if (!card) return;
    dragged = card;
    card.classList.add("dragging");
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  });

  document.addEventListener("dragend", () => {
    if (dragged) dragged.classList.remove("dragging");
    dragged = null;
    clearDrop();
  });

  document.addEventListener("dragover", (e) => {
    if (!dragged) return;
    const body = colBodyOf(e.target);
    if (!body) return;
    e.preventDefault(); // allow the drop
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (body !== dropZone) {
      clearDrop();
      body.classList.add("drop");
      dropZone = body;
    }
  });

  document.addEventListener("drop", (e) => {
    const body = colBodyOf(e.target);
    // `dragged.isConnected` guards the rare case where an SSE morph-patch
    // replaced the card mid-drag: the old node is detached, so skip the move.
    if (!dragged || !dragged.isConnected || !body) {
      clearDrop();
      return;
    }
    e.preventDefault();
    const card = dragged;
    const target = statusOf(body);
    const from = card.dataset.status;
    const id = card.id.replace(/^card-/, "");
    clearDrop();
    if (target && id && target !== from) {
      fetch(`/tickets/${id}/move`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ target }),
      });
    }
  });
})();
