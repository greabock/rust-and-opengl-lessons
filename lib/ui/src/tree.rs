pub use self::shared::{Base, LastResolvedSize, ResizeFlow};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use *;

mod shared {
    use na;
    use std::collections::VecDeque;
    use std::collections::{BTreeMap, BTreeSet};
    use *;

    struct Queues {
        next_queue_id: Ix,
        queues: BTreeMap<Ix, VecDeque<Effect>>,
    }

    impl Queues {
        pub fn new() -> Queues {
            Queues {
                next_queue_id: Ix(0),
                queues: BTreeMap::new(),
            }
        }

        pub fn create_queue(&mut self) -> Ix {
            self.queues.insert(self.next_queue_id, VecDeque::new());
            let id = self.next_queue_id;
            self.next_queue_id.inc();
            id
        }

        pub fn delete_queue(&mut self, id: Ix) {
            self.queues.remove(&id);
        }

        fn send(&mut self, e: Effect) {
            //trace!("event: {:?}", e);

            for (_, q) in self.queues.iter_mut() {
                q.push_back(e);
            }
        }

        pub fn get_queue_mut(&mut self, id: Ix) -> Option<&mut VecDeque<Effect>> {
            self.queues.get_mut(&id)
        }
    }

    struct LayoutingOptions {
        force_equal_child_size: bool,
    }

    impl Default for LayoutingOptions {
        fn default() -> Self {
            LayoutingOptions {
                force_equal_child_size: true,
            }
        }
    }

    #[derive(Copy, Clone, Debug)]
    pub enum LastResolvedSize {
        ElementSizeHidden,
        ElementSizeAuto(Option<ResolvedSize>),
        ElementSizeFixed {
            w: i32,
            h: i32,
            size: Option<ResolvedSize>,
        },
    }

    impl LastResolvedSize {
        pub fn to_box_size(&self) -> BoxSize {
            match *self {
                LastResolvedSize::ElementSizeHidden => BoxSize::Hidden,
                LastResolvedSize::ElementSizeAuto(_) => BoxSize::Auto,
                LastResolvedSize::ElementSizeFixed { w, h, .. } => BoxSize::Fixed { w, h },
            }
        }
    }

    pub struct Base<'a> {
        id: Ix,
        container: &'a mut Container,
        children: &'a mut Children,

        resize_flow: ResizeFlow,
        resize_flow_output: ResizeFlowOutput,
        _box_size: BoxSize,
    }

    impl<'a> Base<'a> {
        fn new<'x>(
            id: Ix,
            container: &'x mut Container,
            children: &'x mut Children,
            resize_flow: ResizeFlow,
            box_size: BoxSize,
        ) -> Base<'x> {
            Base {
                id,
                container,
                children,

                _box_size: box_size,

                resize_flow,
                resize_flow_output: match resize_flow {
                    ResizeFlow::ParentIsResizing => ResizeFlowOutput::ParentIsResizingNoResolve,
                    ResizeFlow::ParentIsNotResizing => {
                        ResizeFlowOutput::ParentIsNotResizingNoSizeUpdate
                    }
                },
            }
        }

        pub fn box_size(&self) -> BoxSize {
            self._box_size
        }

        /// Forces a resize after any update action (excluding the resize).
        pub fn invalidate_size(&mut self) {
            match self.resize_flow {
                ResizeFlow::ParentIsResizing => (),
                ResizeFlow::ParentIsNotResizing => {
                    self.resize_flow_output = ResizeFlowOutput::ParentIsNotResizingSizeInvalidated
                }
            }
        }

        /// Marks the size of this element resolved, and the resize action as executed.
        pub fn resolve_size(&mut self, size: Option<ResolvedSize>) {
            match self.resize_flow {
                ResizeFlow::ParentIsResizing => match size {
                    Some(size) => {
                        self.resize_flow_output = ResizeFlowOutput::ParentIsResizingResolved(size)
                    }
                    None => {
                        self.resize_flow_output = ResizeFlowOutput::ParentIsResizingResolvedNone
                    }
                },
                ResizeFlow::ParentIsNotResizing => self.resize_flow_output = ResizeFlowOutput::ParentIsNotResizingSizeInvalidated,
            }
        }

        pub fn enable_update(&mut self, state: bool) {
            if state {
                self.container
                    .update_set
                    .as_mut()
                    .expect("enable_update (true): self.container.update_set")
                    .insert(self.id);
            } else {
                self.container
                    .update_set
                    .as_mut()
                    .expect("enable_update (false): self.container.update_set")
                    .remove(&self.id);
            }
        }

        pub fn add<E: Element + 'static>(&mut self, element: E) -> Ix {
            let id = self
                .container
                .add_node(self.id, Box::new(element) as Box<Element>);
            self.children.items.insert(id, Child::new(id));
            self.invalidate_size();
            id
        }

        pub fn layout_empty(&mut self) {
            self.children_mut(|_, mut child| {
                child.hide();
            });

            self.resolve_size(None);
        }

        pub fn layout_auto_sized_list(&mut self, margin: i32, flow: FlowDirection) {
            let mut flow_forward = margin;
            let flow_side_offset = margin;

            let mut flow_width = None;

            self.children_mut(|_i, mut child| {
                let actual_size = child.element_resize(BoxSize::Auto);
                if let Some(size) = actual_size {
                    let (element_flow_w, element_flow_val) = size.to_flow(flow);

                    flow_width = match flow_width {
                        None => Some(element_flow_w),
                        Some(w) => if element_flow_w > w {
                            Some(element_flow_w)
                        } else {
                            Some(w)
                        },
                    };

                    child.set_translation(ResolvedSize::from_flow(flow, flow_side_offset, flow_forward));

                    flow_forward += element_flow_val + margin;
                }
            });

            if let Some(w) = flow_width {
                self.resolve_size(Some(ResolvedSize::from_flow(flow, w + margin * 2, flow_forward)));
            } else {
                self.resolve_size(None);
            }
        }

        pub fn layout_equally_sized_fill_list(&mut self, margin: i32, size: ResolvedSize, flow: FlowDirection) {
            let options = LayoutingOptions::default();
            let children_len = self.children_len();

            if children_len == 0 {
                return self.layout_empty();
            }

            let (w, h) = size.to_flow(flow);

            let w_without_margin = w - margin * 2;
            let h_without_margin = h - margin * 2 - margin * (children_len as i32 - 1);

            if w_without_margin <= 0 || h_without_margin <= 0 {
                return self.layout_empty();
            }

            let child_h = h_without_margin / children_len as i32;
            if child_h == 0 {
                return self.layout_empty();
            }

            let mut next_child_offset_y = margin;
            let mut remaining_h = h_without_margin;

            self.children_mut(|i, mut child| {
                let set_w = w_without_margin;
                let set_h = if options.force_equal_child_size {
                    child_h
                } else {
                    if i < children_len {
                        remaining_h -= child_h;
                        child_h
                    } else {
                        remaining_h
                    }
                };

                let offset_y = next_child_offset_y;
                let offset_x = margin;

                let asked_size = ResolvedSize::from_flow(flow, set_w, set_h); ;
                let _actual_size = child.element_resize(BoxSize::Fixed { w: asked_size.w, h: asked_size.h }); // layout ignores actual size

                child.set_translation(ResolvedSize::from_flow(flow, offset_x, offset_y));

                next_child_offset_y += set_h + margin;
            });

            self.resolve_size(Some(ResolvedSize::from_flow(flow, w, h)));
        }

        pub fn layout_vertical(&mut self, margin: i32) {
            match self.box_size() {
                BoxSize::Hidden => self.layout_empty(),
                BoxSize::Auto => self.layout_auto_sized_list(margin, FlowDirection::Vertical),
                BoxSize::Fixed { w, h } => self.layout_equally_sized_fill_list(margin, ResolvedSize { w, h }, FlowDirection::Vertical),
            }
        }

        pub fn layout_horizontal(&mut self, margin: i32) {
            match self.box_size() {
                BoxSize::Hidden => self.layout_empty(),
                BoxSize::Auto => self.layout_auto_sized_list(margin, FlowDirection::Horizontal),
                BoxSize::Fixed { w, h } => self.layout_equally_sized_fill_list(margin, ResolvedSize { w, h }, FlowDirection::Horizontal),
            }
        }

        pub fn children_len(&self) -> usize {
            self.children.items.len()
        }

        pub fn children_mut<F>(&mut self, mut fun: F)
        where
            F: for<'r> FnMut(usize, ChildIterItemMut<'r>),
        {
            // this method uses internal iterator because I failed to make it external

            for (i, (_, child)) in self.children.items.iter_mut().enumerate() {
                fun(
                    i,
                    ChildIterItemMut {
                        child,
                        container: self.container,
                        resize_flow: self.resize_flow,
                    },
                );
            }
        }
    }

    #[derive(Copy, Clone, Debug)]
    pub enum ResizeFlow {
        ParentIsResizing,
        ParentIsNotResizing,
    }

    #[derive(Copy, Clone, Debug)]
    enum ResizeFlowOutput {
        ParentIsResizingNoResolve,
        ParentIsResizingResolvedNone,
        ParentIsResizingResolved(ResolvedSize),
        ParentIsNotResizingNoSizeUpdate,
        ParentIsNotResizingSizeInvalidated,
    }

    pub struct ChildIterItemMut<'a> {
        child: &'a mut Child,
        container: &'a mut Container,
        resize_flow: ResizeFlow,
    }

    impl<'a> ChildIterItemMut<'a> {
        pub fn element_resize(&mut self, size: BoxSize) -> Option<ResolvedSize> {
            self.container.resize(self.child.id, size)
        }

        pub fn set_translation(&mut self, size: ResolvedSize) {
            if size != self.child.translation2d || !self.child.transform_propagated {
                self.child.translation2d = size;
                self.propagate_transform();
            }
        }

        fn propagate_transform(&mut self) {
            let transform = na::Translation3::<f32>::new(
                self.child.translation2d.w as f32,
                self.child.translation2d.h as f32,
                0.0,
            );
            self.container
                .transform(self.child.id, &na::convert(transform));
            self.child.transform_propagated = true;
        }

        pub fn hide(&mut self) {
            self.container.hide(self.child.id);
        }
    }

    pub enum PivotPoint {
        Fractional { x: f32, y: f32 },
        Fixed { x: i32, y: i32 },
    }

    pub struct Child {
        id: Ix,
        translation2d: ResolvedSize,
        rotation2d: f32,
        pivot2d: PivotPoint,
        transform_propagated: bool,
    }

    impl Child {
        pub fn new(id: Ix) -> Child {
            Child {
                id,
                translation2d: ResolvedSize { w: 0, h: 0 },
                rotation2d: 0.0,
                pivot2d: PivotPoint::Fixed { x: 0, y: 0 },
                transform_propagated: false,
            }
        }
    }

    pub struct Children {
        items: BTreeMap<Ix, Child>,
    }

    impl Children {
        pub fn empty() -> Children {
            Children {
                items: BTreeMap::new(),
            }
        }

        pub fn remove(&mut self, id: Ix) {
            self.items.remove(&id);
        }
    }

    pub struct Container {
        queues: Queues,

        next_id: Ix,
        _root_id: Option<Ix>,
        nodes: BTreeMap<Ix, NodeSkeleton>,

        update_set: Option<BTreeSet<Ix>>,
    }

    impl Container {
        pub fn new() -> Container {
            Container {
                queues: Queues::new(),

                next_id: Ix(0),
                _root_id: None,
                nodes: BTreeMap::new(),

                update_set: Some(BTreeSet::new()),
            }
        }

        #[inline(always)]
        fn mutate<IA, I, O, OA, InputFunT, MutFunT, OutputFunT>(
            &mut self,
            id: Ix,
            input_arg: IA,
            mut input_fun: InputFunT, // input_arg comes in, I comes out (access to NodeSkeleton and Queues)
            mut mut_fun: MutFunT,     // I comes in, O comes out (access to Container and Body)
            mut output_fun: OutputFunT, // O comes in, OA is returned (access to NodeSkeleton and Queues)
        ) -> OA
        where
            InputFunT: FnMut(&mut NodeSkeleton, &mut Queues, IA) -> I,
            MutFunT: FnMut(&mut NodeBody, &mut Container, I) -> O,
            OutputFunT: FnMut(&mut NodeSkeleton, &mut Queues, O) -> OA,
        {
            let (mut body, input) = {
                let skeleton = self
                    .nodes
                    .get_mut(&id)
                    .expect("mutate 1: self.nodes.get_mut(&id)");
                let input = input_fun(skeleton, &mut self.queues, input_arg);
                let body = skeleton.steal_body();
                (body, input)
            };

            let output = mut_fun(&mut body, self, input);

            let skeleton = self
                .nodes
                .get_mut(&id)
                .expect("mutate 2: self.nodes.get_mut(&id)");
            skeleton.restore_body(body);

            output_fun(skeleton, &mut self.queues, output)
        }

        pub fn delete_node(&mut self, id: Ix) {
            if let Some(mut removed) = self.nodes.remove(&id) {
                let body = removed.steal_body();

                for (child_id, _) in body.children.items {
                    self.delete_node(child_id);
                }

                let parent_id = removed.parent_id;

                if let Some(parent_id) = parent_id {
                    if let Some(parent) = self.nodes.get_mut(&parent_id) {
                        parent.body.as_mut().expect("delete_node: parent.body")
                            .children.remove(id);
                    }
                }

                self.queues.send(Effect::Remove { id })
            }
        }

        pub fn new_root(&mut self, element: Box<Element>) -> Ix {
            let root_id = self.next_id.inc();

            self.queues.send(Effect::Add {
                id: root_id,
                parent_id: None,
            });

            let mut skeleton =
                NodeSkeleton::new(None,Children::empty(), &na::Projective3::identity(), element);
            let mut body = skeleton.steal_body();

            self.nodes.clear();
            self.nodes.insert(root_id, skeleton);
            self._root_id = Some(root_id);

            {
                let mut base = Base::new(
                    root_id,
                    self,
                    &mut body.children,
                    ResizeFlow::ParentIsNotResizing,
                    BoxSize::Hidden,
                );
                body.el.inflate(&mut base);
            }

            let skeleton = self
                .nodes
                .get_mut(&root_id)
                .expect("new_root: self.nodes.get_mut(&root_id)");
            skeleton.restore_body(body);

            self.queues.send(Effect::Transform {
                id: root_id,
                absolute_transform: na::Projective3::identity(),
            });

            root_id
        }

        pub fn add_node(&mut self, parent_id: Ix, element: Box<Element>) -> Ix {
            let id = self.next_id.inc();

            self.queues.send(Effect::Add {
                id,
                parent_id: Some(parent_id),
            });

            let parent_absolute_transform = self
                .nodes
                .get(&parent_id)
                .expect("add_node 1: self.nodes.get(&parent_id)")
                .absolute_transform();
            let mut skeleton =
                NodeSkeleton::new(Some(parent_id), Children::empty(), &parent_absolute_transform, element);
            let mut body = skeleton.steal_body();

            self.nodes.insert(id, skeleton);

            {
                let mut base = Base::new(
                    id,
                    self,
                    &mut body.children,
                    ResizeFlow::ParentIsNotResizing,
                    BoxSize::Hidden,
                );
                body.el.inflate(&mut base);
            }

            let skeleton = self
                .nodes
                .get_mut(&id)
                .expect("add_node 2: self.nodes.get_mut(&id)");
            skeleton.restore_body(body);

            id
        }

        pub fn root_id(&self) -> Option<Ix> {
            self._root_id
        }

        pub fn get_node_mut(&mut self, id: Ix) -> Option<&mut Element> {
            self.nodes.get_mut(&id).map(|node| node.element_mut())
        }

        pub fn resize(&mut self, id: Ix, box_size: BoxSize) -> Option<ResolvedSize> {
            self.mutate(
                id,
                box_size,
                |skeleton, _q, size| (skeleton.last_resolved_size, size),
                |body, container, (last_resolved_size, box_size)| {
                    match (last_resolved_size, box_size) {
                        (Some(LastResolvedSize::ElementSizeHidden), BoxSize::Hidden) => (LastResolvedSize::ElementSizeHidden, None, true),
                        (Some(LastResolvedSize::ElementSizeAuto(resolved_size)), BoxSize::Auto) => (LastResolvedSize::ElementSizeAuto(resolved_size), resolved_size, true),
                        (Some(LastResolvedSize::ElementSizeFixed { w, h, size }), BoxSize::Fixed { w: new_w, h: new_h }) if w == new_w && h == new_h => (LastResolvedSize::ElementSizeFixed { w, h, size }, size, true),
                        (_, box_size) => {
                            let mut base = Base::new(id, container, &mut body.children, ResizeFlow::ParentIsResizing, box_size);
                            body.el.resize(&mut base);

                            let resolved_size = match base.resize_flow_output {
                                ResizeFlowOutput::ParentIsResizingResolved(size) => Some(size),
                                ResizeFlowOutput::ParentIsResizingResolvedNone => None,
                                ResizeFlowOutput::ParentIsResizingNoResolve => None,
                                ResizeFlowOutput::ParentIsNotResizingSizeInvalidated => unreachable!("resize should not receive ParentIsNotResizing[..] from resize_flow_output"),
                                ResizeFlowOutput::ParentIsNotResizingNoSizeUpdate => unreachable!("resize should not receive ParentIsNotResizing[..] from resize_flow_output"),
                            };

                            (match box_size {
                                BoxSize::Hidden => LastResolvedSize::ElementSizeHidden,
                                BoxSize::Auto => LastResolvedSize::ElementSizeAuto(resolved_size),
                                BoxSize::Fixed { w, h } => LastResolvedSize::ElementSizeFixed { w, h, size: resolved_size },
                            }, resolved_size, false)
                        }
                    }
                },
                |skeleton, q, (last_resolved_size, resolved_size, skip_update)| {
                    if !skip_update {
                        q.send(Effect::Resize { id, size: resolved_size.map(|s| (s.w, s.h)) });
                        skeleton.last_resolved_size = Some(last_resolved_size);
                    }
                    resolved_size
                },
            )
        }

        pub fn transform(&mut self, id: Ix, relative_transform: &na::Projective3<f32>) {
            self.mutate(
                id,
                relative_transform,
                |skeleton, _q, relative_transform| {
                    skeleton.relative_transform = relative_transform.clone();
                    skeleton.absolute_transform()
                },
                |body, container, absolute_transform| {
                    for (child_id, _) in &body.children.items {
                        container.parent_transform(*child_id, &absolute_transform);
                    }
                    absolute_transform
                },
                |_skeleton, q, absolute_transform| {
                    q.send(Effect::Transform {
                        id,
                        absolute_transform,
                    });
                },
            )
        }

        pub fn parent_transform(&mut self, id: Ix, parent_transform: &na::Projective3<f32>) {
            self.mutate(
                id,
                parent_transform,
                |skeleton, _q, parent_transform| {
                    skeleton.parent_transform = parent_transform.clone();
                    skeleton.absolute_transform()
                },
                |body, container, absolute_transform| {
                    for (child_id, _) in &body.children.items {
                        container.parent_transform(*child_id, &absolute_transform);
                    }
                    absolute_transform
                },
                |_skeleton, q, absolute_transform| {
                    q.send(Effect::Transform {
                        id,
                        absolute_transform,
                    });
                },
            )
        }

        pub fn hide(&mut self, id: Ix) {
            self.resize(id, BoxSize::Hidden);
        }

        pub fn create_queue(&mut self) -> Ix {
            self.queues.create_queue()
        }

        pub fn delete_queue(&mut self, id: Ix) {
            self.queues.delete_queue(id);
        }

        pub fn get_queue_mut(&mut self, id: Ix) -> Option<&mut VecDeque<Effect>> {
            self.queues.get_queue_mut(id)
        }

        pub fn update(&mut self, delta: f32) {
            let update_list = ::std::mem::replace(&mut self.update_set, None)
                .expect("update: iteration reentry error");

            enum ResizeAction {
                None,
                InvalidateSize,
            }

            #[derive(Debug)]
            enum ResizeParentsAction {
                None,
                ResizeParents,
            }

            for id in &update_list {
                let (parent_id, post_mutate_resize) = self.mutate(
                    *id,
                    delta,
                    |skeleton, _q, delta| (skeleton.last_resolved_size, delta),
                    |body, container, (last_resolved_size, delta)| {
                        let box_size = match last_resolved_size {
                            None => BoxSize::Hidden,
                            Some(last_resolved_size) => last_resolved_size.to_box_size(),
                        };

                        let resize_flow_output = {
                            let mut base = Base::new(
                                *id, container, &mut body.children,
                                ResizeFlow::ParentIsNotResizing,
                                box_size,
                            );
                            body.el.update(&mut base, delta);
                            base.resize_flow_output
                        };

                        match resize_flow_output {
                            ResizeFlowOutput::ParentIsNotResizingNoSizeUpdate =>
                                ResizeAction::None,
                            ResizeFlowOutput::ParentIsNotResizingSizeInvalidated =>
                                ResizeAction::InvalidateSize,
                            ResizeFlowOutput::ParentIsResizingResolved(_) => unreachable!("non resize should not receive ParentIsResizing[..] from resize_flow_output"),
                            ResizeFlowOutput::ParentIsResizingResolvedNone => unreachable!("non resize should not receive ParentIsResizing[..] from resize_flow_output"),
                            ResizeFlowOutput::ParentIsResizingNoResolve => unreachable!("non resize should not receive ParentIsResizing[..] from resize_flow_output"),
                        }
                    },
                    |skeleton, _q, action| {
                        (skeleton.parent_id, match action {
                            ResizeAction::None => ResizeParentsAction::None,
                            ResizeAction::InvalidateSize => {
                                skeleton.last_resolved_size = None;
                                ResizeParentsAction::ResizeParents
                            },
                        })
                    },
                );

                if let ResizeParentsAction::ResizeParents = post_mutate_resize {
                    // invalidate parent node sizes

                    let mut root = None;
                    let mut parent_id = parent_id;

                    while let Some(id) = parent_id {
                        let node = self.nodes.get_mut(&id)
                            .expect("update: self.nodes.get_mut(&id)");

                        if let Some(_) = node.parent_id {
                            node.last_resolved_size = None;
                        } else {
                            root = Some((id, node.last_resolved_size.map(|s| s.to_box_size())));
                            node.last_resolved_size = None;
                        };

                        parent_id = node.parent_id;
                    }

                    // reflow from root

                    if let Some((root_id, Some(box_size))) = root {
                        self.resize(root_id, box_size);
                    }
                }
            }

            ::std::mem::replace(&mut self.update_set, Some(update_list));
        }
    }

    pub struct NodeBody {
        children: Children,
        el: Box<Element>,
    }

    pub struct NodeSkeleton {
        last_resolved_size: Option<LastResolvedSize>,
        parent_transform: na::Projective3<f32>,
        relative_transform: na::Projective3<f32>,
        parent_id: Option<Ix>,
        body: Option<NodeBody>,
    }

    impl NodeSkeleton {
        pub fn new(
            parent_id: Option<Ix>,
            children: Children,
            parent_transform: &na::Projective3<f32>,
            element: Box<Element>,
        ) -> NodeSkeleton {
            NodeSkeleton {
                last_resolved_size: None,
                parent_transform: parent_transform.clone(),
                relative_transform: na::Projective3::identity(),
                parent_id,
                body: Some(NodeBody {
                    children,
                    el: element,
                }),
            }
        }

        pub fn absolute_transform(&self) -> na::Projective3<f32> {
            &self.parent_transform * &self.relative_transform
        }

        pub fn steal_body(&mut self) -> NodeBody {
            self.body
                .take()
                .expect("steal_body: encountered stolen value")
        }

        pub fn restore_body(&mut self, body: NodeBody) {
            if let Some(_) = ::std::mem::replace(&mut self.body, Some(body)) {
                unreachable!("restore_body: encountered existing value")
            }
        }

        pub fn element_mut(&mut self) -> &mut Element {
            self.body
                .as_mut()
                .map(|b| &mut *b.el)
                .expect("element_mut: encountered stolen value")
        }
    }
}

pub struct Tree {
    shared: Rc<RefCell<shared::Container>>,
}

impl Tree {
    pub fn new() -> Tree {
        let shared = Rc::new(RefCell::new(shared::Container::new()));

        Tree { shared }
    }

    pub fn create_root<T: Element + 'static>(&self, element: T) -> Leaf<T> {
        Leaf {
            _marker: PhantomData,
            id: self
                .shared
                .borrow_mut()
                .new_root(Box::new(element) as Box<Element>),
            shared: self.shared.clone(),
        }
    }

    pub fn update(&self, delta: f32) {
        self.shared.borrow_mut().update(delta)
    }

    pub fn events(&self) -> Events {
        Events::new(&self.shared)
    }
}

pub struct Events {
    queue_id: Ix,
    shared: Rc<RefCell<shared::Container>>,
}

impl Events {
    pub fn new(shared: &Rc<RefCell<shared::Container>>) -> Events {
        Events {
            queue_id: shared.borrow_mut().create_queue(),
            shared: shared.clone(),
        }
    }

    pub fn drain_into(&self, output: &mut Vec<Effect>) {
        let mut shared = self.shared.borrow_mut();
        if let Some(queue) = shared.get_queue_mut(self.queue_id) {
            output.extend(queue.drain(..))
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.shared.borrow_mut().delete_queue(self.queue_id);
    }
}

pub struct Leaf<T> {
    _marker: PhantomData<T>,
    id: Ix,
    shared: Rc<RefCell<shared::Container>>,
}

impl<T> Leaf<T> {
    pub fn resize(&self, size: BoxSize) -> Option<ResolvedSize> {
        self.shared.borrow_mut().resize(self.id, size)
    }
}

impl<T> Drop for Leaf<T> {
    fn drop(&mut self) {
        self.shared.borrow_mut().delete_node(self.id);
    }
}