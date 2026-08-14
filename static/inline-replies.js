(function () {
    "use strict";

    function directReplies(comment) {
        if (!comment) return null;
        const right = Array.from(comment.children).find((child) =>
            child.classList.contains("comment_right")
        );
        if (!right) return null;
        return Array.from(right.children).find((child) =>
            child.classList.contains("replies")
        ) || null;
    }

    function collapsedCommentIds(container) {
        return Array.from(container.querySelectorAll(".comment[id]"))
            .filter((comment) => {
                const right = Array.from(comment.children).find((child) =>
                    child.classList.contains("comment_right")
                );
                return right && !right.open;
            })
            .map((comment) => comment.id);
    }

    function restoreCollapsedComments(ids) {
        ids.forEach((id) => {
            const comment = document.getElementById(id);
            if (!comment) return;
            const right = Array.from(comment.children).find((child) =>
                child.classList.contains("comment_right")
            );
            if (right) right.open = false;
        });
    }

    function toggleCommentFromScore(score) {
        const comment = score.closest(".comment");
        if (!comment) return;
        const right = Array.from(comment.children).find((child) =>
            child.classList.contains("comment_right")
        );
        if (!right) return;
        right.open = !right.open;
        score.setAttribute("aria-expanded", String(right.open));
    }

    document.addEventListener("click", async (event) => {
        const score = event.target.closest(".comment_score[role=\"button\"]");
        if (score) {
            event.preventDefault();
            toggleCommentFromScore(score);
            return;
        }

        const link = event.target.closest("a.deeper_replies");
        if (!link || event.defaultPrevented || event.button !== 0) return;
        if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

        // After a failed inline attempt, preserve the original link fallback.
        if (link.dataset.inlineFailed === "true") return;

        event.preventDefault();
        if (link.dataset.loading === "true") return;

        const parentId = link.dataset.parentId;
        const currentParent = document.getElementById(parentId);
        const currentReplies = directReplies(currentParent);
        if (!parentId || !currentReplies) {
            window.location.assign(link.href);
            return;
        }

        const originalText = link.textContent;
        link.dataset.loading = "true";
        link.setAttribute("aria-busy", "true");
        link.textContent = "Loading replies…";

        try {
            const response = await fetch(link.href, {
                credentials: "same-origin",
                headers: { "X-Redlib-Comments-Only": "1" },
            });
            if (!response.ok) throw new Error(`HTTP ${response.status}`);

            const parsed = new DOMParser().parseFromString(
                await response.text(),
                "text/html"
            );
            const fetchedParent = parsed.getElementById(parentId);
            const fetchedReplies = directReplies(fetchedParent);
            if (!fetchedReplies) throw new Error("Reply branch was not found");

            const collapsedIds = collapsedCommentIds(currentReplies);
            const replacement = document.createDocumentFragment();
            Array.from(fetchedReplies.childNodes).forEach((node) => {
                replacement.appendChild(document.importNode(node, true));
            });
            currentReplies.replaceChildren(replacement);
            restoreCollapsedComments(collapsedIds);
            if (window.redlibRegisterCommentGifs) {
                window.redlibRegisterCommentGifs(currentReplies);
            }
        } catch (error) {
            console.error("Unable to load replies inline:", error);
            link.dataset.inlineFailed = "true";
            link.textContent = "Could not load replies — open page";
            link.removeAttribute("aria-busy");
            link.removeAttribute("data-loading");
            link.title = "Tap again to use the normal page";
            return;
        }

        link.textContent = originalText;
    });

    document.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        const score = event.target.closest(".comment_score[role=\"button\"]");
        if (!score) return;
        event.preventDefault();
        toggleCommentFromScore(score);
    });

    document.addEventListener("toggle", (event) => {
        const right = event.target;
        if (!right.classList || !right.classList.contains("comment_right")) return;
        const comment = right.closest(".comment");
        if (!comment) return;
        const score = Array.from(comment.children)
            .find((child) => child.classList.contains("comment_left"))
            ?.querySelector(".comment_score[role=\"button\"]");
        if (score) score.setAttribute("aria-expanded", String(right.open));
    }, true);
})();
