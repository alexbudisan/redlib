(function () {
    "use strict";

    const script = document.currentScript;
    const avoidMetered = script && script.dataset.avoidMetered === "on";
    const connection = navigator.connection || navigator.mozConnection || navigator.webkitConnection;
    const maximumPreviews = Math.max(0, Math.min(50,
        Number.parseInt(script?.dataset.maximumPreviews || "10", 10) || 0
    ));
    const maximumBytes = Math.max(1, Math.min(100,
        Number.parseInt(script?.dataset.maximumMegabytes || "40", 10) || 40
    )) * 1024 * 1024;
    let registeredPreviews = 0;
    let loadedBytes = 0;
    let loadQueue = Promise.resolve();
    const registered = new WeakSet();

    function isReportedMetered() {
        if (!avoidMetered || !connection) return false;
        return connection.saveData === true ||
            connection.type === "cellular" ||
            connection.effectiveType === "slow-2g" ||
            connection.effectiveType === "2g";
    }

    function suppress(media, reason) {
        media.removeAttribute("data-src");
        media.classList.add("comment_gif_suppressed");
        const label = media.parentElement && media.parentElement.querySelector(".comment_gif_label");
        if (label) label.textContent = reason;
    }

    async function loadMedia(media) {
        if (isReportedMetered()) {
            suppress(media, "GIF preview disabled on this connection — tap for source");
            return;
        }

        const source = media.dataset.src;
        if (!source) return;
        media.removeAttribute("data-src");

        try {
            const response = await fetch(source, { credentials: "same-origin" });
            if (!response.ok) throw new Error(`HTTP ${response.status}`);

            const declaredBytes = Number.parseInt(response.headers.get("content-length") || "0", 10);
            if (declaredBytes > 0 && loadedBytes + declaredBytes > maximumBytes) {
                if (response.body) response.body.cancel();
                suppress(media, "GIF preview omitted (page size limit) — tap for source");
                return;
            }

            const blob = await response.blob();
            if (loadedBytes + blob.size > maximumBytes) {
                suppress(media, "GIF preview omitted (page size limit) — tap for source");
                return;
            }

            loadedBytes += blob.size;
            // Safari is unreliable when animated images are swapped to large
            // in-memory blob URLs. Reuse the normal proxied URL instead. The
            // completed fetch has already populated the HTTP cache, so this
            // should not require another backend download.
            media.src = source;
        } catch (error) {
            console.error("Unable to load comment GIF:", error);
            suppress(media, "GIF preview unavailable — tap for source");
        }
    }

    function queueMedia(media) {
        loadQueue = loadQueue.then(() => loadMedia(media));
    }

    const observer = "IntersectionObserver" in window
        ? new IntersectionObserver((entries) => {
            entries.forEach((entry) => {
                if (!entry.isIntersecting) return;
                const media = entry.target;
                observer.unobserve(media);
                queueMedia(media);
            });
        }, { rootMargin: "200px 0px" })
        : null;

    function register(root) {
        root.querySelectorAll(".comment_gif[data-src]").forEach((media) => {
            if (registered.has(media)) return;
            registered.add(media);

            if (isReportedMetered()) {
                suppress(media, "GIF preview disabled on this connection — tap for source");
                return;
            }
            if (registeredPreviews >= maximumPreviews) {
                suppress(media, "GIF preview omitted (page limit) — tap for source");
                return;
            }

            registeredPreviews += 1;
            if (observer) {
                observer.observe(media);
            } else {
                queueMedia(media);
            }
        });
    }

    window.redlibRegisterCommentGifs = register;
    register(document);
})();
