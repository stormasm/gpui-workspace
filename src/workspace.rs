use std::{
    cmp,
    collections::{hash_map, HashMap},
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

use crate::{
    dock::{Panel, PanelHandle},
    pane_group,
};
use anyhow::Result;
use gpui::{
    actions, canvas, div, impl_internal_actions, prelude::FluentBuilder as _, AnyWeakView, App,
    AppContext, Bounds, Context, Div, DragMoveEvent, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyContext, ParentElement as _, Pixels, Point,
    Render, Styled as _, Subscription, Task, WeakEntity, Window,
};
use serde::Deserialize;
use ui::{h_flex, theme::ActiveTheme};

use super::{
    dock::{Dock, DockPosition},
    pane::{self, Pane},
    pane_group::{PaneGroup, SplitDirection},
};

actions!(
    workspace,
    [
        ActivateNextPane,
        ActivatePreviousPane,
        CloseAllDocks,
        ToggleBottomDock,
        ToggleCenteredLayout,
        ToggleLeftDock,
        ToggleRightDock,
        ToggleZoom,
        CloseAllItemsAndPanes,
        CloseInactiveTabsAndPanes,
        ReopenClosedItem,
    ]
);

#[derive(Clone, Deserialize, PartialEq)]
pub struct ActivatePane(pub usize);

#[derive(Clone, Deserialize, PartialEq)]
pub struct ActivatePaneInDirection(pub SplitDirection);

#[derive(Clone, Deserialize, PartialEq)]
pub struct SwapPaneInDirection(pub SplitDirection);

impl_internal_actions!(
    workspace,
    [ActivatePane, ActivatePaneInDirection, SwapPaneInDirection,]
);

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceId(i64);

enum ActivateInDirectionTarget {
    Pane(Entity<Pane>),
    Dock(Entity<Dock>),
}

/// Workspace is a container for docks.
#[allow(clippy::type_complexity)]
pub struct Workspace {
    weak_self: WeakEntity<Self>,
    center: PaneGroup,
    left_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    panes: Vec<Entity<Pane>>,
    pub(crate) panes_by_item: HashMap<EntityId, WeakEntity<Pane>>,
    active_pane: Entity<Pane>,
    last_active_center_pane: Option<WeakEntity<Pane>>,
    pub(crate) zoomed: Option<AnyWeakView>,
    pub(crate) zoomed_position: Option<DockPosition>,
    database_id: Option<WorkspaceId>,
    bounds: Bounds<Pixels>,
    workspace_actions: Vec<Box<dyn Fn(Div, &mut Window, &mut Context<Self>) -> Div>>,
    bounds_save_task_queued: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

pub enum Event {
    PaneAdded(Entity<Pane>),
    PaneRemoved,
    ItemAdded,
    ItemRemoved,
    ActiveItemChanged,
    WorkspaceCreated(WeakEntity<Workspace>),
    ZoomChanged,
}

impl EventEmitter<Event> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active_pane.focus_handle(cx)
    }
}

#[derive(Clone, Render)]
pub struct DraggedDock(pub DockPosition);

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut context = KeyContext::new_with_defaults();
        context.add("Workspace");

        // let render_padding = |size| {
        //     (size > 0.0).then(|| {
        //         div()
        //             .h_full()
        //             .w(relative(size))
        //             .bg(cx.theme().background)
        //             .border_color(cx.theme().border)
        //     })
        // };

        self.actions(div(), window, cx)
            .key_context(context)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(cx.theme().font_family.clone())
            .gap_0()
            .justify_start()
            .items_start()
            .text_color(cx.theme().foreground)
            .bg(cx.theme().background)
            // .children(self.titlebar_item.clone())
            .child(
                div()
                    .id("workspace")
                    .relative()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child({
                        let this = cx.entity().clone();
                        canvas(
                            move |bounds, _, cx| this.update(cx, |this, _cx| this.bounds = bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .when(self.zoomed.is_none(), |this| {
                        this.on_drag_move(cx.listener(
                            |workspace, e: &DragMoveEvent<DraggedDock>, window, cx| {
                                match e.drag(cx).0 {
                                    DockPosition::Left => {
                                        let size = workspace.bounds.left() + e.event.position.x;
                                        workspace.left_dock.update(cx, |left_dock, cx| {
                                            left_dock.resize_active_panel(Some(size), window, cx);
                                        });
                                    }
                                    DockPosition::Right => {
                                        let size = workspace.bounds.right() - e.event.position.x;
                                        workspace.right_dock.update(cx, |right_dock, cx| {
                                            right_dock.resize_active_panel(Some(size), window, cx);
                                        });
                                    }
                                    DockPosition::Bottom => {
                                        let size = workspace.bounds.bottom() - e.event.position.y;
                                        workspace.bottom_dock.update(cx, |bottom_dock, cx| {
                                            bottom_dock.resize_active_panel(Some(size), window, cx);
                                        });
                                    }
                                }
                            },
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .h_full()
                            // Left Dock
                            .children(self.zoomed_position.ne(&Some(DockPosition::Left)).then(
                                || {
                                    div()
                                        .flex()
                                        .flex_none()
                                        .overflow_hidden()
                                        .child(self.left_dock.clone())
                                },
                            ))
                            // Panes
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(h_flex().flex_1().child(self.center.render(
                                        &self.active_pane,
                                        self.zoomed.as_ref(),
                                        window,
                                        cx,
                                    )))
                                    .children(
                                        self.zoomed_position
                                            .ne(&Some(DockPosition::Bottom))
                                            .then(|| self.bottom_dock.clone()),
                                    ),
                            )
                            // Right Dock
                            .children(self.zoomed_position.ne(&Some(DockPosition::Right)).then(
                                || {
                                    div()
                                        .flex()
                                        .flex_none()
                                        .overflow_hidden()
                                        .child(self.right_dock.clone())
                                },
                            )),
                    )
                    .children(self.zoomed.as_ref().and_then(|view| {
                        let zoomed_view = view.upgrade()?;
                        let div = div()
                            .occlude()
                            .absolute()
                            .overflow_hidden()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().background)
                            .child(zoomed_view)
                            .inset_0()
                            .shadow_lg();

                        Some(match self.zoomed_position {
                            Some(DockPosition::Left) => div.right_2().border_r_1(),
                            Some(DockPosition::Right) => div.left_2().border_l_1(),
                            Some(DockPosition::Bottom) => div.top_2().border_t_1(),
                            None => div.top_2().bottom_2().left_2().right_2().border_1(),
                        })
                    })),
            )
    }
}

impl Workspace {
    pub fn new(
        workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_focus_lost(window, |this, window, cx| {
            let focus_handle = this.focus_handle(cx);
            window.focus(&focus_handle);
        })
        .detach();

        let weak_handle = cx.entity().downgrade();
        let _pane_history_timestamp = Arc::new(AtomicUsize::new(0));

        let center_pane = cx.new(|cx| Pane::new(weak_handle.clone(), None, window, cx));
        cx.subscribe_in(&center_pane, window, Self::handle_pane_event)
            .detach();
        cx.focus_view(&center_pane, window);
        cx.emit(Event::PaneAdded(center_pane.clone()));
        // let window_handle = cx.window_handle().downcast::<Workspace>().unwrap();

        cx.emit(Event::WorkspaceCreated(weak_handle.clone()));
        let left_dock = Dock::new(DockPosition::Left, window, cx);
        let bottom_dock = Dock::new(DockPosition::Bottom, window, cx);
        let right_dock = Dock::new(DockPosition::Right, window, cx);
        // let left_dock_buttons = cx.new_view(|cx| PanelButtons::new(left_dock.clone(), cx));
        // let bottom_dock_buttons = cx.new_view(|cx| PanelButtons::new(bottom_dock.clone(), cx));
        // let right_dock_buttons = cx.new_view(|cx| PanelButtons::new(right_dock.clone(), cx));

        let subscriptions = vec![
            cx.observe_window_activation(window, Self::on_window_activation_changed),
            cx.observe_window_bounds(window, move |this, window, cx| {
                if this.bounds_save_task_queued.is_some() {
                    return;
                }
                this.bounds_save_task_queued =
                    Some(cx.spawn_in(window, |this, mut cx| async move {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        this.update_in(&mut cx, |this, window, cx| {
                            if let Some(display) = window.display(cx) {
                                if let Ok(_display_uuid) = display.uuid() {
                                    let _window_bounds = window.window_bounds();
                                    if let Some(_database_id) = workspace_id {
                                        // cx.background_executor()
                                        //     .spawn(DB.set_window_open_status(
                                        //         database_id,
                                        //         SerializedWindowBounds(window_bounds),
                                        //         display_uuid,
                                        //     ))
                                        //     .detach_and_log_err(cx);
                                    }
                                }
                            }
                            this.bounds_save_task_queued.take();
                        })
                        .ok();
                    }));
                cx.notify();
            }),
            cx.observe_in(&left_dock, window, |this, _, window, cx| {
                this.serialize_workspace(window, cx);
                cx.notify();
            }),
            cx.observe_in(&bottom_dock, window, |this, _, window, cx| {
                this.serialize_workspace(window, cx);
                cx.notify();
            }),
            cx.observe_in(&right_dock, window, |this, _, window, cx| {
                this.serialize_workspace(window, cx);
                cx.notify();
            }),
            // cx.on_release(|this, window, cx| {
            //     this.app_state.workspace_store.update(cx, |store, _| {
            //         let window = window.downcast::<Self>().unwrap();
            //         store.workspaces.remove(&window);
            //     })
            // }),
        ];

        Workspace {
            weak_self: weak_handle.clone(),
            zoomed: None,
            zoomed_position: None,
            center: PaneGroup::new(center_pane.clone()),
            panes: vec![center_pane.clone()],
            panes_by_item: Default::default(),
            active_pane: center_pane.clone(),
            last_active_center_pane: Some(center_pane.downgrade()),
            left_dock,
            bottom_dock,
            right_dock,
            database_id: workspace_id,
            workspace_actions: Default::default(),
            // This data will be incorrect, but it will be overwritten by the time it needs to be used.
            bounds: Default::default(),
            bounds_save_task_queued: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn on_window_activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            if let Some(_database_id) = self.database_id {
                // cx.background_executor()
                //     .spawn(persistence::DB.update_timestamp(database_id))
                //     .detach();
            }
        } else {
            for pane in &self.panes {
                pane.update(cx, |pane, cx| {
                    if let Some(item) = pane.active_item() {
                        item.workspace_deactivated(window, cx);
                    }
                    // for item in pane.items() {
                    //     if matches!(
                    //         item.workspace_settings(cx).autosave,
                    //         AutosaveSetting::OnWindowChange | AutosaveSetting::OnFocusChange
                    //     ) {
                    //         Pane::autosave_item(item.as_ref(), self.project.clone(), cx)
                    //             .detach_and_log_err(cx);
                    //     }
                    // }
                });
            }
        }
    }

    fn add_workspace_actions_listeners(
        &self,
        div: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut div = div
            .on_action(cx.listener(Self::close_inactive_items_and_panes))
            .on_action(cx.listener(Self::close_all_items_and_panes));

        for action in self.workspace_actions.iter() {
            div = (action)(div, window, cx)
        }
        div
    }

    fn actions(&self, div: Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.add_workspace_actions_listeners(div, window, cx)
            .on_action(cx.listener(Self::close_inactive_items_and_panes))
            .on_action(cx.listener(Self::close_all_items_and_panes))
            .on_action(
                cx.listener(|workspace, _: &ActivatePreviousPane, window, cx| {
                    workspace.activate_previous_pane(window, cx)
                }),
            )
            .on_action(cx.listener(|workspace, _: &ActivateNextPane, window, cx| {
                workspace.activate_next_pane(window, cx)
            }))
            .on_action(
                cx.listener(|workspace, action: &ActivatePaneInDirection, window, cx| {
                    workspace.activate_pane_in_direction(action.0, window, cx)
                }),
            )
            .on_action(
                cx.listener(|workspace, action: &SwapPaneInDirection, window, cx| {
                    workspace.swap_pane_in_direction(action.0, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| {
                this.toggle_dock(DockPosition::Left, window, cx);
            }))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ToggleRightDock, window, cx| {
                    workspace.toggle_dock(DockPosition::Right, window, cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ToggleBottomDock, window, cx| {
                    workspace.toggle_dock(DockPosition::Bottom, window, cx);
                },
            ))
            .on_action(
                cx.listener(|workspace: &mut Workspace, _: &CloseAllDocks, window, cx| {
                    workspace.close_all_docks(window, cx);
                }),
            )
            .on_action(cx.listener(Workspace::activate_pane_at_index))
            .on_action(cx.listener(
                |_workspace: &mut Workspace, _: &ReopenClosedItem, _window, _cx| {
                    // workspace.reopen_closed_item(cx).detach();
                },
            ))
    }

    pub fn add_panel<T: Panel>(&mut self, panel: Entity<T>, window: &mut Window, cx: &mut App) {
        let dock = match panel.position(window, cx) {
            DockPosition::Left => &self.left_dock,
            DockPosition::Bottom => &self.bottom_dock,
            DockPosition::Right => &self.right_dock,
        };

        dock.update(cx, |dock, cx| {
            dock.add_panel(panel, self.weak_self.clone(), window, cx)
        });
    }

    pub fn close_inactive_items_and_panes(
        &mut self,
        _action: &CloseInactiveTabsAndPanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self.close_all_internal(true, window, cx) {
            task.detach_and_log_err(cx)
        }
    }

    pub fn close_all_items_and_panes(
        &mut self,
        _action: &CloseAllItemsAndPanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self.close_all_internal(false, window, cx) {
            task.detach_and_log_err(cx)
        }
    }

    fn close_all_internal(
        &mut self,
        retain_active_pane: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let current_pane = self.active_pane();

        let mut tasks = Vec::new();

        if retain_active_pane {
            if let Some(current_pane_close) = current_pane.update(cx, |pane, cx| {
                pane.close_inactive_items(&pane::CloseInactiveItems, window, cx)
            }) {
                tasks.push(current_pane_close);
            };
        }

        for pane in self.panes() {
            if retain_active_pane && pane.entity_id() == current_pane.entity_id() {
                continue;
            }

            if let Some(close_pane_items) = pane.update(cx, |pane: &mut Pane, cx| {
                pane.close_all_items(&pane::CloseAllItems, window, cx)
            }) {
                tasks.push(close_pane_items)
            }
        }

        if tasks.is_empty() {
            None
        } else {
            Some(cx.spawn(|_, _| async move {
                for task in tasks {
                    task.await?
                }
                Ok(())
            }))
        }
    }

    pub fn weak_handle(&self) -> WeakEntity<Self> {
        self.weak_self.clone()
    }

    pub fn left_dock(&self) -> &Entity<Dock> {
        &self.left_dock
    }

    pub fn bottom_dock(&self) -> &Entity<Dock> {
        &self.bottom_dock
    }

    pub fn right_dock(&self) -> &Entity<Dock> {
        &self.right_dock
    }

    pub fn database_id(&self) -> Option<WorkspaceId> {
        self.database_id
    }

    fn add_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let pane = cx.new(|cx| Pane::new(self.weak_handle(), None, window, cx));
        cx.subscribe_in(&pane, window, Self::handle_pane_event)
            .detach();
        self.panes.push(pane.clone());
        cx.focus_view(&pane, window);
        cx.emit(Event::PaneAdded(pane.clone()));
        pane
    }

    pub fn split_pane(
        &mut self,
        pane_to_split: Entity<Pane>,
        split_direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let new_pane = self.add_pane(window, cx);
        self.center
            .split(&pane_to_split, &new_pane, split_direction)
            .unwrap();
        cx.notify();
        new_pane
    }

    pub fn split_and_clone(
        &mut self,
        pane: Entity<Pane>,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        let item = pane.read(cx).active_item()?;
        let maybe_pane_handle =
            if let Some(clone) = item.clone_on_split(self.database_id(), window, cx) {
                let new_pane = self.add_pane(window, cx);
                new_pane.update(cx, |pane, cx| {
                    pane.add_item(clone, true, true, None, window, cx)
                });
                self.center.split(&pane, &new_pane, direction).unwrap();
                Some(new_pane)
            } else {
                None
            };
        cx.notify();
        maybe_pane_handle
    }

    pub fn split_pane_with_item(
        &mut self,
        pane_to_split: WeakEntity<Pane>,
        split_direction: SplitDirection,
        from: WeakEntity<Pane>,
        item_id_to_move: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_to_split) = pane_to_split.upgrade() else {
            return;
        };
        let Some(from) = from.upgrade() else {
            return;
        };

        let new_pane = self.add_pane(window, cx);
        self.move_item(
            from.clone(),
            new_pane.clone(),
            item_id_to_move,
            0,
            window,
            cx,
        );
        self.center
            .split(&pane_to_split, &new_pane, split_direction)
            .unwrap();
        cx.notify();
    }

    pub fn move_item(
        &mut self,
        source: Entity<Pane>,
        destination: Entity<Pane>,
        item_id_to_move: EntityId,
        destination_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((item_ix, item_handle)) = source
            .read(cx)
            .items()
            .enumerate()
            .find(|(_, item_handle)| item_handle.item_id() == item_id_to_move)
        else {
            // Tab was closed during drag
            return;
        };

        let item_handle = item_handle.clone();

        if source != destination {
            // Close item from previous pane
            source.update(cx, |source, cx| {
                source.remove_item(item_ix, false, window, cx);
            });
        }

        // This automatically removes duplicate items in the pane
        destination.update(cx, |destination, cx| {
            destination.add_item(item_handle, true, true, Some(destination_index), window, cx);
            destination.focus(window)
        });
    }

    fn remove_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        if self.center.remove(pane).unwrap() {
            self.force_remove_pane(pane, window, cx);

            for removed_item in pane.read(cx).items() {
                self.panes_by_item.remove(&removed_item.item_id());
            }

            cx.notify();
        }

        cx.emit(Event::PaneRemoved);
    }

    fn force_remove_pane(
        &mut self,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        self.panes.retain(|p| p != pane);
        self.panes
            .last()
            .unwrap()
            .update(cx, |pane, _| pane.focus(window));
        if self.last_active_center_pane == Some(pane.downgrade()) {
            self.last_active_center_pane = None;
        }
        cx.notify();
    }

    pub fn panes(&self) -> &[Entity<Pane>] {
        &self.panes
    }

    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.active_pane
    }

    // pub fn reopen_closed_item(&mut self, window: &mut Window, cx: &mut Context<Workspace>) -> Task<Result<()>> {
    //     self.navigate_history(
    //         self.active_pane().downgrade(),
    //         NavigationMode::ReopeningClosedItem,
    //         cx,
    //     )
    // }

    fn activate_pane_at_index(
        &mut self,
        action: &ActivatePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.center.panes();
        if let Some(pane) = panes.get(action.0).map(|p| (*p).clone()) {
            cx.focus_view(&pane, window);
        } else {
            self.split_and_clone(self.active_pane.clone(), SplitDirection::Right, window, cx);
        }
    }

    pub fn activate_next_pane(&mut self, window: &mut Window, cx: &mut App) {
        let panes = self.center.panes();
        if let Some(ix) = panes.iter().position(|pane| **pane == self.active_pane) {
            let next_ix = (ix + 1) % panes.len();
            let next_pane = panes[next_ix].clone();
            window.focus(&next_pane.focus_handle(cx));
        }
    }

    pub fn activate_previous_pane(&mut self, window: &mut Window, cx: &mut App) {
        let panes = self.center.panes();
        if let Some(ix) = panes.iter().position(|pane| **pane == self.active_pane) {
            let prev_ix = cmp::min(ix.wrapping_sub(1), panes.len() - 1);
            let prev_pane = panes[prev_ix].clone();
            window.focus(&prev_pane.focus_handle(cx));
        }
    }

    pub fn activate_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut App,
    ) {
        use ActivateInDirectionTarget as Target;
        enum Origin {
            LeftDock,
            RightDock,
            BottomDock,
            Center,
        }

        let origin: Origin = [
            (&self.left_dock, Origin::LeftDock),
            (&self.right_dock, Origin::RightDock),
            (&self.bottom_dock, Origin::BottomDock),
        ]
        .into_iter()
        .find_map(|(dock, origin)| {
            if dock.focus_handle(cx).contains_focused(window, cx) && dock.read(cx).is_open() {
                Some(origin)
            } else {
                None
            }
        })
        .unwrap_or(Origin::Center);

        let get_last_active_pane = || {
            self.last_active_center_pane.as_ref().and_then(|p| {
                let p = p.upgrade()?;
                (p.read(cx).items_len() != 0).then_some(p)
            })
        };

        let try_dock =
            |dock: &Entity<Dock>| dock.read(cx).is_open().then(|| Target::Dock(dock.clone()));

        let target = match (origin, direction) {
            // We're in the center, so we first try to go to a different pane,
            // otherwise try to go to a dock.
            (Origin::Center, direction) => {
                if let Some(pane) = self.find_pane_in_direction(direction, window, cx) {
                    Some(Target::Pane(pane))
                } else {
                    match direction {
                        SplitDirection::Up => None,
                        SplitDirection::Down => try_dock(&self.bottom_dock),
                        SplitDirection::Left => try_dock(&self.left_dock),
                        SplitDirection::Right => try_dock(&self.right_dock),
                    }
                }
            }

            (Origin::LeftDock, SplitDirection::Right) => {
                if let Some(last_active_pane) = get_last_active_pane() {
                    Some(Target::Pane(last_active_pane))
                } else {
                    try_dock(&self.bottom_dock).or_else(|| try_dock(&self.right_dock))
                }
            }

            (Origin::LeftDock, SplitDirection::Down)
            | (Origin::RightDock, SplitDirection::Down) => try_dock(&self.bottom_dock),

            (Origin::BottomDock, SplitDirection::Up) => get_last_active_pane().map(Target::Pane),
            (Origin::BottomDock, SplitDirection::Left) => try_dock(&self.left_dock),
            (Origin::BottomDock, SplitDirection::Right) => try_dock(&self.right_dock),

            (Origin::RightDock, SplitDirection::Left) => {
                if let Some(last_active_pane) = get_last_active_pane() {
                    Some(Target::Pane(last_active_pane))
                } else {
                    try_dock(&self.bottom_dock).or_else(|| try_dock(&self.left_dock))
                }
            }

            _ => None,
        };

        match target {
            Some(ActivateInDirectionTarget::Pane(pane)) => window.focus(&pane.focus_handle(cx)),
            Some(ActivateInDirectionTarget::Dock(dock)) => {
                if let Some(panel) = dock.read(cx).active_panel() {
                    panel.panel_focus_handle(cx).focus(window);
                } else {
                    log::error!("Could not find a focus target when in switching focus in {direction} direction for a {:?} dock", dock.read(cx).position());
                }
            }
            None => {}
        }
    }

    pub fn find_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        _window: &Window,
        cx: &App,
    ) -> Option<Entity<Pane>> {
        let bounding_box = self.center.bounding_box_for_pane(&self.active_pane)?;
        let cursor = self.active_pane.read(cx).pixel_position_of_cursor(cx);
        let center = match cursor {
            Some(cursor) if bounding_box.contains(&cursor) => cursor,
            _ => bounding_box.center(),
        };

        let distance_to_next = pane_group::HANDLE_HITBOX_SIZE;

        let target = match direction {
            SplitDirection::Left => {
                Point::new(bounding_box.left() - distance_to_next.into(), center.y)
            }
            SplitDirection::Right => {
                Point::new(bounding_box.right() + distance_to_next.into(), center.y)
            }
            SplitDirection::Up => {
                Point::new(center.x, bounding_box.top() - distance_to_next.into())
            }
            SplitDirection::Down => {
                Point::new(center.x, bounding_box.bottom() + distance_to_next.into())
            }
        };
        self.center.pane_at_pixel_position(target).cloned()
    }

    pub fn swap_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(to) = self.find_pane_in_direction(direction, window, cx) {
            self.center.swap(&self.active_pane.clone(), &to);
            cx.notify();
        }
    }

    fn handle_pane_focused(
        &mut self,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_pane != pane {
            self.active_pane = pane.clone();
            self.last_active_center_pane = Some(pane.downgrade());
        }

        self.dismiss_zoomed_items_to_reveal(None, window, cx);
        if pane.read(cx).is_zoomed() {
            self.zoomed = Some(pane.downgrade().into());
        } else {
            self.zoomed = None;
        }
        self.zoomed_position = None;
        cx.emit(Event::ZoomChanged);

        cx.notify();
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &pane::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            pane::Event::AddItem { item } => {
                item.added_to_pane(self, pane, window, cx);
                cx.emit(Event::ItemAdded);
            }
            pane::Event::Split(direction) => {
                self.split_and_clone(pane.clone(), *direction, window, cx);
            }
            pane::Event::Remove => self.remove_pane(pane, window, cx),
            pane::Event::ActivateItem { local: _ } => {
                cx.emit(Event::ActiveItemChanged);
            }
            pane::Event::ChangeItemTitle => {
                cx.emit(Event::ActiveItemChanged);
            }
            pane::Event::RemoveItem { item_id } => {
                cx.emit(Event::ActiveItemChanged);

                if let hash_map::Entry::Occupied(entry) = self.panes_by_item.entry(*item_id) {
                    if entry.get().entity_id() == pane.entity_id() {
                        entry.remove();
                    }
                }
            }
            pane::Event::Focus => {
                self.handle_pane_focused(pane.clone(), window, cx);
            }
            pane::Event::ZoomIn => {
                if *pane == self.active_pane {
                    pane.update(cx, |pane, cx| pane.set_zoomed(true, window, cx));
                    if pane.read(cx).has_focus(window, cx) {
                        self.zoomed = Some(pane.downgrade().into());
                        self.zoomed_position = None;
                        cx.emit(Event::ZoomChanged);
                    }
                    cx.notify();
                }
            }
            pane::Event::ZoomOut => {
                pane.update(cx, |pane, cx| pane.set_zoomed(false, window, cx));
                if self.zoomed_position.is_none() {
                    self.zoomed = None;
                    cx.emit(Event::ZoomChanged);
                }
                cx.notify();
            }
        }

        self.serialize_workspace(window, cx);
    }

    pub fn toggle_dock(
        &mut self,
        dock_side: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock = match dock_side {
            DockPosition::Left => &self.left_dock,
            DockPosition::Bottom => &self.bottom_dock,
            DockPosition::Right => &self.right_dock,
        };
        let mut focus_center = false;
        let mut reveal_dock = false;
        dock.update(cx, |dock, cx| {
            let other_is_zoomed = self.zoomed.is_some() && self.zoomed_position != Some(dock_side);
            let was_visible = dock.is_open() && !other_is_zoomed;
            dock.set_open(!was_visible, window, cx);

            if let Some(active_panel) = dock.active_panel() {
                if was_visible {
                    if active_panel
                        .panel_focus_handle(cx)
                        .contains_focused(window, cx)
                    {
                        focus_center = true;
                    }
                } else {
                    let focus_handle = &active_panel.panel_focus_handle(cx);
                    window.focus(focus_handle);
                    reveal_dock = true;
                }
            }
        });

        if reveal_dock {
            self.dismiss_zoomed_items_to_reveal(Some(dock_side), window, cx);
        }

        if focus_center {
            self.active_pane.update(cx, |pane, _| pane.focus(window))
        }

        cx.notify();
        self.serialize_workspace(window, cx);
    }

    pub fn close_all_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let docks = [&self.left_dock, &self.bottom_dock, &self.right_dock];

        for dock in docks {
            dock.update(cx, |dock, cx| {
                dock.set_open(false, window, cx);
            });
        }

        cx.focus_self(window);
        cx.notify();
        self.serialize_workspace(window, cx);
    }

    fn dismiss_zoomed_items_to_reveal(
        &mut self,
        dock_to_reveal: Option<DockPosition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If a center pane is zoomed, unzoom it.
        for pane in &self.panes {
            if pane != &self.active_pane || dock_to_reveal.is_some() {
                pane.update(cx, |pane, cx| pane.set_zoomed(false, window, cx));
            }
        }

        // If another dock is zoomed, hide it.
        let mut focus_center = false;
        for dock in [&self.left_dock, &self.right_dock, &self.bottom_dock] {
            dock.update(cx, |dock, cx| {
                if Some(dock.position()) != dock_to_reveal {
                    if let Some(panel) = dock.active_panel() {
                        if panel.is_zoomed(window, cx) {
                            focus_center |=
                                panel.panel_focus_handle(cx).contains_focused(window, cx);
                            dock.set_open(false, window, cx);
                        }
                    }
                }
            });
        }

        if focus_center {
            self.active_pane.update(cx, |pane, _| pane.focus(window))
        }

        if self.zoomed_position != dock_to_reveal {
            self.zoomed = None;
            self.zoomed_position = None;
            cx.emit(Event::ZoomChanged);
        }

        cx.notify();
    }

    pub(crate) fn serialize_workspace(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        // if self._schedule_serialize.is_none() {
        //     self._schedule_serialize = Some(cx.spawn(|this, mut cx| async move {
        //         cx.background_executor()
        //             .timer(Duration::from_millis(100))
        //             .await;
        //         this.update(&mut cx, |this, cx| {
        //             this.serialize_workspace_internal(cx).detach();
        //             this._schedule_serialize.take();
        //         })
        //         .log_err();
        //     }));
        // }
    }
}
