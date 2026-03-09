//! TrackedDiv — wraps any element and records its bounds in BoundsRegistry.
//!
//! GPUI's `.id()` changes `Div` → `Stateful<Div>` (different type), so we can't
//! use the standard annotator approach. TrackedDiv implements the GPUI `Element`
//! trait directly, delegating layout/paint to the child while recording bounds
//! in `prepaint()`.

use gpui::*;

use crate::geometry::BoundsRegistry;

pub struct TrackedDiv {
    id: String,
    registry: BoundsRegistry,
    child: AnyElement,
}

impl TrackedDiv {
    pub fn new(id: impl Into<String>, registry: BoundsRegistry, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            registry,
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for TrackedDiv {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TrackedDiv {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.child.request_layout(window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.registry.record(self.id.clone(), bounds);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
