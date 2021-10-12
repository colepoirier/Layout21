//!
//! # Cell Definition
//!
//! Defines the [Cell] type, which represents a multi-viewed piece of reusable hardware.
//! [Cell]s can, and generally do, have one or more associated "views",
//! including [Abstract]s, [Layout], interface definitions, and/or "raw" layouts.
//!

// Crates.io
use derive_more;

// Local imports
use crate::bbox::{BoundBox, HasBoundBox};
use crate::coords::{PrimPitches, Xy};
use crate::placement::{Place, Placeable};
use crate::raw::{Dir, LayoutError, LayoutResult};
use crate::stack::{Assign, RelZ};
use crate::utils::{Ptr, PtrList};
use crate::{abs, interface, outline, raw, tracks};

/// # Layout Cell Implementation
///
/// A combination of lower-level cell instances and net-assignments to tracks.
///
#[derive(Debug, Clone, Builder)]
#[builder(pattern = "owned", setter(into))]
pub struct Layout {
    /// Cell Name
    pub name: String,
    /// Number of Metal Layers Used
    pub metals: usize,
    /// Outline shape, counted in x and y pitches of `stack`
    pub outline: outline::Outline,

    /// Layout Instances
    #[builder(default)]
    pub instances: PtrList<Instance>,
    /// Net-to-track assignments
    #[builder(default)]
    pub assignments: Vec<Assign>,
    /// Track cuts
    #[builder(default)]
    pub cuts: Vec<tracks::TrackIntersection>,
    /// Placeable objects
    #[builder(default)]
    pub places: Vec<Placeable>,
}
impl Layout {
    /// Create a new [Layout]
    pub fn new(name: impl Into<String>, metals: usize, outline: outline::Outline) -> Self {
        let name = name.into();
        Layout {
            name,
            metals,
            outline,
            instances: PtrList::new(),
            assignments: Vec::new(),
            cuts: Vec::new(),
            places: Vec::new(),
        }
    }
    /// Create a [LayoutBuilder], a struct created by the [Builder] macro.
    pub fn builder() -> LayoutBuilder {
        LayoutBuilder::default()
    }
    /// Assign a net at the given coordinates.
    pub fn assign(
        &mut self,
        net: impl Into<String>,
        layer: usize,
        track: usize,
        at: usize,
        relz: RelZ,
    ) {
        let net = net.into();
        self.assignments.push(Assign {
            net,
            at: tracks::TrackIntersection {
                layer,
                track,
                at,
                relz,
            },
        })
    }
    /// Add a cut at the specified coordinates.
    pub fn cut(&mut self, layer: usize, track: usize, at: usize, relz: RelZ) {
        self.cuts.push(tracks::TrackIntersection {
            layer,
            track,
            at,
            relz,
        })
    }
    /// Get a temporary handle for net assignments
    pub fn net<'h>(&'h mut self, net: impl Into<String>) -> NetHandle<'h> {
        let name = net.into();
        NetHandle { name, parent: self }
    }
}
/// A short-term handle for chaining multiple assignments to a net
/// Typically used as: `mycell.net("name").at(/* args */).at(/* more args */)`
/// Takes an exclusive reference to its parent [Layout],
/// so generally must be dropped quickly to avoid locking it up.
pub struct NetHandle<'h> {
    name: String,
    parent: &'h mut Layout,
}
impl<'h> NetHandle<'h> {
    /// Assign our net at the given coordinates.
    /// Consumes and returns `self` to enable chaining.
    pub fn at(self, layer: usize, track: usize, at: usize, relz: RelZ) -> Self {
        self.parent.assign(&self.name, layer, track, at, relz);
        self
    }
}
/// "Pointer" to a raw (lib, cell) combination.
/// Wraps with basic [Outline] and `metals` information to enable bounded placement.
#[derive(Debug, Clone)]
pub struct RawLayoutPtr {
    /// Outline shape, counted in x and y pitches of `stack`
    pub outline: outline::Outline,
    /// Number of Metal Layers Used
    pub metals: usize,
    /// Pointer to the raw Library
    pub lib: Ptr<raw::Library>,
    /// Pointer to the raw Cell
    pub cell: Ptr<raw::Cell>,
}
/// # Cell View Enumeration
/// All of the ways in which a Cell is represented
#[derive(derive_more::From, Debug, Clone)]
pub enum CellView {
    Interface(interface::Bundle),
    Abstract(abs::Abstract),
    Layout(Layout),
    RawLayoutPtr(RawLayoutPtr),
}

/// Collection of the Views describing a Cell
#[derive(Debug, Default, Clone)]
pub struct Cell {
    /// Cell Name
    pub name: String,
    /// Interface
    pub interface: Option<interface::Bundle>,
    /// Layout Abstract
    pub abs: Option<abs::Abstract>,
    /// Layout Implementation
    pub layout: Option<Layout>,
    /// Raw Layout
    /// FIXME: this should probably move "up" a level,
    /// so that cells are either defined as `raw` or `tetris` implementations,
    /// but not both
    pub raw: Option<RawLayoutPtr>,
}
impl Cell {
    /// Create a new and initially empty [Cell]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
    /// Add [CellView] `view` to our appropriate type-based field.
    pub fn add_view(&mut self, view: impl Into<CellView>) {
        let view = view.into();
        match view {
            CellView::Interface(x) => {
                self.interface.replace(x);
            }
            CellView::Abstract(x) => {
                self.abs.replace(x);
            }
            CellView::Layout(x) => {
                self.layout.replace(x);
            }
            CellView::RawLayoutPtr(x) => {
                self.raw.replace(x);
            }
        }
    }
    /// Create from a list of [CellView]s and a name.
    pub fn from_views(name: impl Into<String>, views: Vec<CellView>) -> Self {
        let mut myself = Self::default();
        myself.name = name.into();
        for view in views {
            myself.add_view(view);
        }
        myself
    }
    /// Return whichever view highest-prioritorily dictates the outline
    pub fn outline(&self) -> LayoutResult<&outline::Outline> {
        // We take the "most abstract" view for the outline
        // (although if there are more than one, they better be the same...
        // FIXME: this should be a validation step.)
        // Overall this method probably should move to a "validated" cell in which each view is assured consistent.
        if let Some(ref x) = self.abs {
            Ok(&x.outline)
        } else if let Some(ref x) = self.layout {
            Ok(&x.outline)
        } else if let Some(ref x) = self.raw {
            Ok(&x.outline)
        } else {
            Err(LayoutError::Validation)
        }
    }
    /// Size of the [Cell]'s rectangular `boundbox`.
    pub fn boundbox_size(&self) -> LayoutResult<Xy<PrimPitches>> {
        let outline = self.outline()?;
        Ok(Xy::new(outline.xmax(), outline.ymax()))
    }
    /// Return whichever view highest-prioritorily dictates the top-layer
    pub fn metals(&self) -> LayoutResult<usize> {
        // FIXME: same commentary as `outline` above
        if let Some(ref x) = self.abs {
            Ok(x.metals)
        } else if let Some(ref x) = self.layout {
            Ok(x.metals)
        } else if let Some(ref x) = self.raw {
            Ok(x.metals)
        } else {
            Err(LayoutError::Validation)
        }
    }
    /// Get the cell's top metal layer (numer).
    /// Returns `None` if no metal layers are used.
    pub fn top_metal(&self) -> LayoutResult<Option<usize>> {
        let metals = self.metals()?;
        if metals == 0 {
            Ok(None)
        } else {
            Ok(Some(metals - 1))
        }
    }
}
impl From<CellView> for Cell {
    fn from(src: CellView) -> Self {
        match src {
            CellView::Interface(x) => x.into(),
            CellView::Abstract(x) => x.into(),
            CellView::Layout(x) => x.into(),
            CellView::RawLayoutPtr(x) => x.into(),
        }
    }
}
impl From<interface::Bundle> for Cell {
    fn from(src: interface::Bundle) -> Self {
        Self {
            name: src.name.clone(),
            interface: Some(src),
            ..Default::default()
        }
    }
}
impl From<abs::Abstract> for Cell {
    fn from(src: abs::Abstract) -> Self {
        Self {
            name: src.name.clone(),
            abs: Some(src),
            ..Default::default()
        }
    }
}
impl From<Layout> for Cell {
    fn from(src: Layout) -> Self {
        Self {
            name: src.name.clone(),
            layout: Some(src),
            ..Default::default()
        }
    }
}
impl From<RawLayoutPtr> for Cell {
    fn from(src: RawLayoutPtr) -> Self {
        let name = {
            let cell = src.cell.read().unwrap();
            cell.name.clone()
        };
        Self {
            name,
            raw: Some(src),
            ..Default::default()
        }
    }
}

/// Instance of another Cell
#[derive(Debug, Clone)]
pub struct Instance {
    /// Instance Name
    pub inst_name: String,
    /// Cell Definition Reference
    pub cell: Ptr<Cell>,
    /// Location of the Instance origin
    /// This origin-position holds regardless of either `reflect` field.
    /// If specified in absolute coordinates, location-units are [PrimPitches].
    pub loc: Place<Xy<PrimPitches>>,
    /// Horizontal Reflection
    pub reflect_horiz: bool,
    /// Vertical Reflection
    pub reflect_vert: bool,
}
impl Instance {
    /// Boolean indication of whether this Instance is reflected in direction `dir`
    pub fn reflected(&self, dir: Dir) -> bool {
        match dir {
            Dir::Horiz => self.reflect_horiz,
            Dir::Vert => self.reflect_vert,
        }
    }
    /// Size of the Instance's rectangular `boundbox`, i.e. the zero-origin `boundbox` of its `cell`.
    pub fn boundbox_size(&self) -> LayoutResult<Xy<PrimPitches>> {
        let cell = self.cell.read()?;
        cell.boundbox_size()
    }
}
impl std::fmt::Display for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cell_name = {
            let cell = self.cell.read().unwrap();
            cell.name.clone()
        };
        write!(
            f,
            "Instance(name={}, cell={}, loc={:?})",
            self.inst_name, cell_name, self.loc
        )
    }
}
impl HasBoundBox for Instance {
    type Units = PrimPitches;
    type Error = LayoutError;
    /// Retrieve this Instance's bounding rectangle, specified in [PrimPitches].
    /// Instance location must be resolved to absolute coordinates, or this method will fail.
    fn boundbox(&self) -> LayoutResult<BoundBox<PrimPitches>> {
        let loc = self.loc.abs()?;
        let cell = self.cell.read()?;
        let outline = cell.outline()?;
        let (x0, x1) = match self.reflect_horiz {
            false => (loc.x, loc.x + outline.xmax()),
            true => (loc.x - outline.xmax(), loc.x),
        };
        let (y0, y1) = match self.reflect_vert {
            false => (loc.y, loc.y + outline.ymax()),
            true => (loc.y - outline.ymax(), loc.y),
        };
        Ok(BoundBox::new(Xy::new(x0, y0), Xy::new(x1, y1)))
    }
}
