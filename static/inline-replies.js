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

// On phones, every nesting level costs indentation, so deep replies end up a
// few characters wide. Past MAX_DEPTH, a comment shows "Continue this thread"
// instead of its replies. Tapping it shows that branch on its own at full
// width, using the replies already on the page (no request to Reddit). The
// back link or the browser's back gesture returns to the full thread.
(function () {
    "use strict";

    const MAX_DEPTH = 7;
    const narrow = window.matchMedia("(max-width: 800px)");
    const column = document.getElementById("column_one");
    // One entry per focused branch, innermost last.
    const stack = [];

    if (!column) return;

    function childWithClass(element, className) {
        return element
            ? Array.from(element.children).find((child) => child.classList.contains(className)) || null
            : null;
    }

    function repliesOf(comment) {
        return childWithClass(childWithClass(comment, "comment_right"), "replies");
    }

    // Nesting level relative to the enclosing thread or focused branch.
    function depthOf(comment) {
        let depth = 0;
        for (let el = comment.parentElement; el && el !== column; el = el.parentElement) {
            if (el.classList.contains("thread") || el.classList.contains("thread_focus")) break;
            if (el.classList.contains("replies")) depth += 1;
        }
        return depth;
    }

    function permalinkOf(comment) {
        const created = comment.querySelector(":scope > .comment_right > .comment_data .created");
        return (created && created.dataset.permalink) || "#";
    }

    function update() {
        column.querySelectorAll(".comment[id]").forEach((comment) => {
            const replies = repliesOf(comment);
            const cut = narrow.matches && depthOf(comment) === MAX_DEPTH && replies && replies.querySelector(".comment, .deeper_replies");
            const existing = childWithClass(childWithClass(comment, "comment_right"), "continue_thread");

            comment.classList.toggle("thread_cut", Boolean(cut));
            if (!cut) {
                if (existing) existing.remove();
                return;
            }

            const count = replies.querySelectorAll(".comment").length;
            const label = count > 0 ? `Continue this thread (${count} more) →` : "Continue this thread →";
            if (existing) {
                if (existing.textContent !== label) existing.textContent = label;
                return;
            }
            const link = document.createElement("a");
            link.className = "continue_thread";
            link.href = permalinkOf(comment);
            link.textContent = label;
            replies.before(link);
        });
    }

    function scrollToElement(element) {
        // Leave room for the fixed navbar when that setting is on.
        window.scrollTo(0, element.getBoundingClientRect().top + window.scrollY - 60);
    }

    function focusBranch(comment) {
        const placeholder = document.createComment("continued thread");
        comment.before(placeholder);

        const view = document.createElement("div");
        view.className = "thread_focus";
        const back = document.createElement("a");
        back.className = "thread_focus_back";
        back.href = "#";
        back.textContent = stack.length ? "← Back" : "← Back to full thread";
        view.append(back, comment);

        if (stack.length) stack[stack.length - 1].view.hidden = true;
        column.append(view);
        stack.push({ comment, placeholder, view, scrollY: window.scrollY });
        document.body.classList.add("thread_focused");
        history.pushState({ redlibThreadFocus: stack.length }, "");

        update();
        scrollToElement(view);
    }

    function unfocusBranch() {
        const entry = stack.pop();
        if (!entry) return;
        entry.placeholder.replaceWith(entry.comment);
        entry.view.remove();
        if (stack.length) {
            stack[stack.length - 1].view.hidden = false;
        } else {
            document.body.classList.remove("thread_focused");
        }
        update();
        window.scrollTo(0, entry.scrollY);
    }

    document.addEventListener("click", (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

        const back = event.target.closest("a.thread_focus_back");
        if (back) {
            event.preventDefault();
            history.back();
            return;
        }

        const link = event.target.closest("a.continue_thread");
        if (!link) return;
        const comment = link.closest(".comment");
        if (!comment) return;
        event.preventDefault();
        focusBranch(comment);
    });

    window.addEventListener("popstate", (event) => {
        const target = (event.state && event.state.redlibThreadFocus) || 0;
        while (stack.length > target) unfocusBranch();
    });

    // Replies loaded inline can push comments past MAX_DEPTH.
    let pending = false;
    new MutationObserver(() => {
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
            pending = false;
            update();
        });
    }).observe(column, { childList: true, subtree: true });

    narrow.addEventListener("change", update);
    update();
})();
